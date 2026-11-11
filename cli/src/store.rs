//! Reading and writing the todo.txt file. Lines are kept exactly as they are in the file
//! (blank lines included) so that line numbers match what an editor shows.

use std::fs;
use std::io;
use std::path::Path;

pub struct Store {
    pub lines: Vec<String>,
    crlf: bool,
}

impl Store {
    /// A missing file is an empty list (it is created on the first save).
    pub fn load(path: &Path) -> io::Result<Store> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let crlf = text.contains("\r\n");
                let lines = text.lines().map(str::to_string).collect();
                Ok(Store { lines, crlf })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Store { lines: Vec::new(), crlf: cfg!(windows) }),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        save_lines(path, &self.lines, self.crlf)
    }

    /// The file content as it would be written.
    pub fn text(&self) -> String {
        render(&self.lines, self.crlf)
    }

    pub fn crlf(&self) -> bool {
        self.crlf
    }

    /// 1-based task number -> index, only for non-blank lines.
    pub fn index_of(&self, number: usize) -> Option<usize> {
        let i = number.checked_sub(1)?;
        self.lines.get(i).filter(|l| !l.trim().is_empty()).map(|_| i)
    }
}

pub fn render(lines: &[String], crlf: bool) -> String {
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(nl);
    if !lines.is_empty() {
        out.push_str(nl);
    }
    out
}

pub fn save_lines(path: &Path, lines: &[String], crlf: bool) -> io::Result<()> {
    let out = render(lines, crlf);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    fs::write(path, out)
}
