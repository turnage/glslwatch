//! glslwatch provides a live glsl source with include support.
//!

//! Construct the GLSL source tree by passing the shader path and a vec
//! of paths to search for included files.
//! ```
//! let include_dirs: Vec<String> = vec!["shaders/include"];
//! let src_tree = GLSLTree::new("shaders/frag.glsl", include_dirs)?;
//! ```
//!
//! The fully rendered tree is cached in memory and we can retrieve it with
//! `.render()`.
//! ```
//! let src_str = src_tree.render();
//! ```
//!
//! We can refresh the tree if it is expired.
//! ```
//! let src_tree = if src_tree.expired()? {
//!     src_tree.refresh()?
//! } else {
//!     src_tree
//! };
//! ```
//!

extern crate failure;
extern crate itertools;
#[macro_use]
extern crate lazy_static;
#[cfg(test)]
#[macro_use]
extern crate maplit;
extern crate regex;
extern crate rpds;

mod preprocess;

use failure::Fail;
use rpds::List;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::path::Path;

use preprocess::AnnotatedGLSL;

type Result<T> = std::result::Result<T, Error>;

/// An error loading or refreshing a GLSL source tree.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    FailedToOpen {
        path: String,
        searched_dirs: Vec<String>,
        cause: std::io::Error,
    },
    Cycle(List<String>),
    VersionMismatch {
        root_version: usize,
        src_version: usize,
        src_path: String,
    },
    MissingRoot,
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> std::result::Result<(), std::fmt::Error> {
        match *self {
            Error::Io(ref e) => write!(f, "IO error of undecidede relevance: {}", e),
            Error::FailedToOpen {
                ref path,
                ref searched_dirs,
                ref cause,
            } => write!(
                f,
                "Failed to open {}; searched in all of: {:?}; cause: {}",
                path, searched_dirs, cause
            ),
            Error::Cycle(ref branch) => {
                write!(f, "Source tree has a cycle in branch: {:?}", branch)
            }
            Error::VersionMismatch {
                ref root_version,
                ref src_version,
                ref src_path
            } => write!(f, "Version mismatch: root has version {} (110 is the default version) but {} has version {}", root_version, src_path, src_version),
            Error::MissingRoot => write!(f, "No or empty path given for root shader."),
        }
    }
}

impl Fail for Error {
    fn cause(&self) -> Option<&Fail> {
        match *self {
            Error::Io(ref e) => Some(e),
            Error::FailedToOpen { ref cause, .. } => Some(cause),
            _ => None,
        }
    }
}

/// An in-memory GLSL source tree.
#[derive(Debug, Clone)]
pub struct GLSLTree {
    include_dirs: Vec<String>,
    src_map: HashMap<String, AnnotatedGLSL>,
    root_path: String,
    rendered: String,
}

impl GLSLTree {
    /// Creates a GLSL source tree from the given glsl file, tracing all its include directives
    /// and looking for the included files in all given include directories.
    ///
    /// If an include is ambiguous, the first file found will be loaded, so take care of your
    /// include directory order if this applies to you.
    pub fn new<P: AsRef<Path>, P2: AsRef<Path>>(path: P, include_dirs: &[P2]) -> Result<Self> {
        Self::with_default_version(path, include_dirs, 110)
    }

    /// Works like `new`, except sets the default version. By default OpenGL assumes GLSL
    /// source without a version pragma is version 110. You can pass another default version
    /// to this constructor, but the root source's explicit version pragma if it has one will
    /// always override the default.
    pub fn with_default_version<P: AsRef<Path>, P2: AsRef<Path>>(
        path: P,
        include_dirs: &[P2],
        default_version: usize,
    ) -> Result<Self> {
        let root_path = match path.as_ref().to_str() {
            Some(s) => Ok(String::from(s)),
            None => Err(Error::MissingRoot),
        }?;
        let include_dirs: Vec<String> = include_dirs
            .into_iter()
            .filter_map(|dir| dir.as_ref().to_str().map(String::from))
            .collect();

        let src_map = GLSLTree::build_node(
            &root_path,
            &include_dirs,
            &List::new(),
            None,
            HashMap::new(),
        )?;

        let lines = GLSLTree::render_node(
            &src_map.get(&root_path).unwrap(),
            &src_map,
            &mut HashSet::new(),
        );
        let version: usize = src_map
            .get(&root_path)
            .unwrap()
            .version_pragma
            .map(|(_, v)| v)
            .unwrap_or(default_version);
        let rendered = vec![version]
            .into_iter()
            .map(|v| format!("#version {}", v))
            .chain(lines.into_iter())
            .collect::<Vec<String>>()
            .join("\n");

        Ok(GLSLTree {
            include_dirs,
            rendered,
            src_map,
            root_path,
        })
    }

    /// Refreshes the source tree from disk, re-tracing from the root. Only files
    /// still included in the source tree will be present in the refreshed cache.
    pub fn refresh(self) -> Result<Self> {
        Self::new(self.root_path, &self.include_dirs)
    }

    /// Returns whether one or more nodes of the cached source tree are out of sync with
    /// the filesystem.
    pub fn expired(&self) -> Result<bool> {
        Ok(self.src_map
            .iter()
            .map(|(_, ref src)| -> Result<bool> { src.expired() })
            .collect::<Result<Vec<bool>>>()?
            .into_iter()
            .any(|e| e))
    }

    /// Returns the cached source string, whith all includes processed.
    /// This is the result you should feed into your GLSL compiler.
    pub fn render<'a>(&'a self) -> &'a str {
        &self.rendered
    }

    fn build_node(
        path: &String,
        include_dirs: &[String],
        branch: &List<String>,
        version: Option<usize>,
        mut src_map: HashMap<String, AnnotatedGLSL>,
    ) -> Result<HashMap<String, AnnotatedGLSL>> {
        let src = if branch.is_empty() {
            // root shader; don't search include dirs.
            AnnotatedGLSL::load(path, &Vec::<String>::new())
        } else {
            AnnotatedGLSL::load(path, &include_dirs)
        }.and_then(|src| match (version, src.version_pragma) {
            (Some(root_version), Some((_, src_version))) if root_version != src_version => {
                Err(Error::VersionMismatch {
                    root_version,
                    src_version,
                    src_path: path.clone(),
                })
            }
            _ => Ok(src),
        })?;

        let version = if branch.is_empty() {
            // root shader; default GLSL version is 110 if no version pragma.
            src.version_pragma.map(|(_, v)| v).or(Some(110))
        } else {
            version
        };

        let branch = branch.push_front(path.clone());
        let include_files = src.includes
            .clone()
            .into_iter()
            .map(|(_, v)| v)
            .map(|included_file| {
                if branch.iter().any(|p| included_file == *p) {
                    Err(Error::Cycle(branch.push_front(included_file.clone())))
                } else {
                    Ok(included_file)
                }
            })
            .collect::<Result<Vec<String>>>()?;
        src_map.insert(path.clone(), src);
        include_files.into_iter().fold(
            Ok(src_map),
            move |src_map_r: Result<HashMap<_, _>>,
                  included_file: String|
                  -> Result<HashMap<_, _>> {
                let branch = branch.clone();
                src_map_r.and_then(move |src_map| {
                    GLSLTree::build_node(&included_file, include_dirs, &branch, version, src_map)
                })
            },
        )
    }

    fn render_node(
        src: &AnnotatedGLSL,
        src_map: &HashMap<String, AnnotatedGLSL>,
        seen: &mut HashSet<String>,
    ) -> Vec<String> {
        src.lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                if let Some((path, ref src)) = src.includes
                    .get(&i)
                    .and_then(|path| src_map.get(path).map(|src| (path, src)))
                {
                    if seen.contains(path) {
                        None
                    } else {
                        seen.insert(path.clone());
                        Some(GLSLTree::render_node(src, src_map, seen))
                    }
                } else if let Some(true) = src.version_pragma.map(|(j, _)| j == i) {
                    None
                } else {
                    Some(vec![line.clone()])
                }
            })
            .filter_map(|v| v)
            .flat_map(|v| v)
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn it_works() {
        let tree = GLSLTree::new("src/test_glsl/simple.vert", &["src/test_glsl"]).expect("my tree");
        println!("render: {}", tree.render());
    }
}
