use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Task {
    pub id: u32,
    pub text: String,
    pub due: Option<NaiveDate>,
    pub tags: Vec<String>,
    pub done: bool,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct TodoFile {
    pub version: u32,
    pub next_id: u32,
    pub tasks: Vec<Task>,
}

impl Default for TodoFile {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            tasks: Vec::new(),
        }
    }
}

impl TodoFile {
    pub fn add(&mut self, text: String, due: Option<NaiveDate>, tags: Vec<String>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(Task {
            id,
            text,
            due,
            tags,
            done: false,
        });
        id
    }

    pub fn done(&mut self, id: u32) -> Result<()> {
        match self.tasks.iter_mut().find(|t| t.id == id) {
            Some(task) => {
                task.done = true;
                Ok(())
            }
            None => bail!("no task with id {id}"),
        }
    }

    pub fn remove(&mut self, id: u32) -> Result<()> {
        let Some(pos) = self.tasks.iter().position(|t| t.id == id) else {
            bail!("no task with id {id}");
        };
        self.tasks.remove(pos);
        Ok(())
    }
}

/// A missing file is an empty list; a file that does not parse is an error and stays as it is.
pub fn load(path: &Path) -> Result<TodoFile> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(TodoFile::default()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    serde_json::from_str(&text)
        .with_context(|| format!("{} is not a valid todo file", path.display()))
}

/// Writes a sibling `.tmp` file, then renames it over `path`, so a crash never leaves half a file.
pub fn save(path: &Path, file: &TodoFile) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let json = serde_json::to_string_pretty(file)? + "\n";
    fs::write(&tmp, json).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("todo-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_is_empty() {
        let path = tmp("missing").join("todo.json");
        assert_eq!(load(&path).unwrap(), TodoFile::default());
    }

    #[test]
    fn round_trip() {
        let path = tmp("round").join("todo.json");
        let mut file = TodoFile::default();
        file.add(
            "Buy milk".into(),
            NaiveDate::from_ymd_opt(2026, 11, 2),
            vec!["home".into()],
        );
        file.add("Write spec".into(), None, vec![]);
        save(&path, &file).unwrap();
        assert_eq!(load(&path).unwrap(), file);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"version\": 1")
        );
    }

    #[test]
    fn ids_not_reused_after_remove() {
        let mut file = TodoFile::default();
        assert_eq!(file.add("a".into(), None, vec![]), 1);
        assert_eq!(file.add("b".into(), None, vec![]), 2);
        file.remove(2).unwrap();
        assert_eq!(file.add("c".into(), None, vec![]), 3);
    }

    #[test]
    fn corrupt_file_errors_and_is_untouched() {
        let path = tmp("corrupt").join("todo.json");
        fs::write(&path, "{not json").unwrap();
        let err = load(&path).unwrap_err();
        assert!(format!("{err:#}").contains(&path.display().to_string()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{not json");
    }

    #[test]
    fn unknown_id_errors() {
        let mut file = TodoFile::default();
        assert_eq!(file.done(9).unwrap_err().to_string(), "no task with id 9");
        assert_eq!(file.remove(9).unwrap_err().to_string(), "no task with id 9");
    }

    #[test]
    fn done_twice_is_ok() {
        let mut file = TodoFile::default();
        file.add("a".into(), None, vec![]);
        file.done(1).unwrap();
        file.done(1).unwrap();
        assert!(file.tasks[0].done);
    }

    #[test]
    fn stale_tmp_does_not_break_save() {
        let dir = tmp("stale");
        let path = dir.join("todo.json");
        fs::write(dir.join("todo.json.tmp"), "junk").unwrap();
        let mut file = TodoFile::default();
        file.add("a".into(), None, vec![]);
        save(&path, &file).unwrap();
        assert_eq!(load(&path).unwrap(), file);
    }

    #[test]
    fn save_creates_parent_folder() {
        let path = tmp("parent").join("a").join("b").join("todo.json");
        save(&path, &TodoFile::default()).unwrap();
        assert!(path.exists());
    }
}
