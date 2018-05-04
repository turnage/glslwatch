use Result;
use regex::Regex;
use std::collections::HashMap;
use std::io::Read;
use std::fs::File;
use std::time::SystemTime;
use std::path::PathBuf;

lazy_static! {
    static ref INCLUDE_RE: Regex = Regex::new(r#"("|<)([^">]+)("|>)$"#).unwrap();
    static ref VERSION_RE: Regex = Regex::new(r#"\s*(\d\d\d)\s*$"#).unwrap();
}

#[derive(Debug, PartialEq, Clone)]
pub struct AnnotatedGLSL {
    pub lines: Vec<String>,
    pub version_pragma: Option<(usize, usize)>,
    pub includes: HashMap<usize, String>,
    pub mtime: SystemTime,
    pub path: String,
}

impl AnnotatedGLSL {
    pub fn load(path: &str, search_dirs: &[String]) -> Result<AnnotatedGLSL> {
        let (mut file, found_path) = search_dirs.iter().fold(
            File::open(&path).map(|f| (f, PathBuf::from(String::from(path)))),
            |r, include_dir| {
                r.or_else(|_| {
                    let mut prefixed_path = PathBuf::new();
                    prefixed_path.push(&include_dir);
                    prefixed_path.push(&path);
                    Ok((File::open(&prefixed_path)?, prefixed_path))
                })
            },
        )?;
        let mut src = String::new();
        let _ = file.read_to_string(&mut src)?;

        let lines: Vec<String> = src.lines().map(String::from).collect();
        let mut version_pragma = None;
        let mut includes = HashMap::new();
        for i in 0..(lines.len()) {
            match directive(&lines[i]) {
                Some(Directive::Version(version)) => version_pragma = Some((i, version)),
                Some(Directive::Include(path)) => {
                    includes.insert(i, path);
                }
                None => (),
            };
        }
        Ok(AnnotatedGLSL {
            lines,
            version_pragma,
            includes,
            mtime: file.metadata()?.modified()?,
            path: String::from(found_path.to_str().unwrap()),
        })
    }

    pub fn expired(&self) -> Result<bool> {
        Ok(self.mtime < File::open(&self.path)?.metadata()?.modified()?)
    }
}

#[derive(Debug)]
enum Directive {
    Version(usize),
    Include(String),
}

fn directive(line: &str) -> Option<Directive> {
    if let Some((i, '#')) = line.chars()
        .enumerate()
        .skip_while(|&(_, c)| c.is_whitespace())
        .next()
    {
        match line.get((i + 1)..(i + 8)) {
            Some("include") => match line.get((i + 9)..)
                .and_then(|s| INCLUDE_RE.captures(s))
                .and_then(|c| c.get(2))
            {
                Some(path) => Some(Directive::Include(String::from(path.as_str()))),
                None => None,
            },
            Some("version") => match line.get((i + 9)..)
                .and_then(|s| VERSION_RE.captures(s))
                .and_then(|c| c.get(1))
            {
                Some(version) => Some(Directive::Version(
                    version.as_str().parse::<usize>().unwrap(),
                )),
                None => None,
            },
            _ => None,
        }
    } else {
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn hello() {
        let result = AnnotatedGLSL::load(
            "src/test_glsl/simple.vert",
            &[String::from("src/test_glsl")],
        ).expect("annotated glsl");
        assert_eq!(result.version_pragma, Some((0, 150)));
        assert_eq!(result.includes, hashmap!{1 => String::from("common.vert")});

        let expiry = result.expired().expect("expiry");
        assert_eq!(expiry, false);
    }
}
