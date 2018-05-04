extern crate failure;
extern crate itertools;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate maplit;
extern crate regex;
extern crate rpds;

mod preprocess;

use failure::Fail;
use std::path::Path;
use std::fmt::{Display, Formatter};
use std::collections::HashMap;
use rpds::List;

use preprocess::AnnotatedGLSL;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    MissingRoot,
    Cycle(List<String>),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> std::result::Result<(), std::fmt::Error> {
        write!(f, "Error")
    }
}

impl Fail for Error {}

#[derive(Debug, Clone)]
pub struct GLSLTree {
    include_dirs: Vec<String>,
    src_map: HashMap<String, AnnotatedGLSL>,
    root_path: String,
    rendered: String,
}

impl GLSLTree {
    pub fn new<P: AsRef<Path>, P2: AsRef<Path>>(path: P, include_dirs: &[P2]) -> Result<Self> {
        let root_path = match path.as_ref().to_str() {
            Some(s) => Ok(String::from(s)),
            None => Err(Error::MissingRoot),
        }?;
        let include_dirs: Vec<String> = include_dirs
            .into_iter()
            .filter_map(|dir| dir.as_ref().to_str().map(String::from))
            .collect();

        let src_map =
            GLSLTree::build_node(&root_path, &include_dirs, &List::new(), HashMap::new())?;
        Ok(GLSLTree {
            include_dirs,
            rendered: GLSLTree::render_node(&src_map.get(&root_path).unwrap(), &src_map),
            src_map,
            root_path,
        })
    }

    pub fn refresh(self) -> Result<Self> {
        Self::new(self.root_path, &self.include_dirs)
    }

    pub fn expired(&self) -> Result<bool> {
        Ok(self.src_map
            .iter()
            .map(|(_, ref src)| -> Result<bool> { src.expired() })
            .collect::<Result<Vec<bool>>>()?
            .into_iter()
            .any(|e| e))
    }

    pub fn render<'a>(&'a self) -> &'a str {
        &self.rendered
    }

    pub fn build_node(
        path: &String,
        include_dirs: &[String],
        branch: &List<String>,
        mut src_map: HashMap<String, AnnotatedGLSL>,
    ) -> Result<HashMap<String, AnnotatedGLSL>> {
        let src = if branch.is_empty() {
            // root shader; don't search include dirs.
            AnnotatedGLSL::load(path, &Vec::<String>::new())
        } else {
            AnnotatedGLSL::load(path, &include_dirs)
        }?;

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
                    GLSLTree::build_node(&included_file, include_dirs, &branch, src_map)
                })
            },
        )
    }

    fn render_node(node: &AnnotatedGLSL, src_map: &HashMap<String, AnnotatedGLSL>) -> String {
        node.lines
            .iter()
            .enumerate()
            .flat_map(|(i, line)| match node.includes.get(&i) {
                Some(ref path) => src_map
                    .get(*path)
                    .map(|ref src| src.lines.clone())
                    .unwrap_or(Vec::new()),
                None => vec![line.clone()],
            })
            .collect::<Vec<String>>()
            .join("\n")
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
