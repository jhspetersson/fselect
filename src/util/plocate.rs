use std::path::PathBuf;
use std::process::Command;

#[derive(Debug)]
pub enum PlocateError {
    Spawn,
    Failed(Option<i32>),
}

pub fn query_descendants(root_abspath: &str) -> Result<Vec<PathBuf>, PlocateError> {
    let mut prefix = root_abspath.to_string();
    if !prefix.ends_with('/') {
        prefix.push('/');
    }

    let output = Command::new("plocate")
        .arg("--null")
        .arg(&prefix)
        .output()
        .map_err(|_| PlocateError::Spawn)?;

    if !output.status.success() {
        match output.status.code() {
            Some(0) | Some(1) => {}
            other => return Err(PlocateError::Failed(other)),
        }
    }

    use std::os::unix::ffi::OsStrExt;
    let mut results = Vec::new();
    for chunk in output.stdout.split(|&b| b == 0) {
        if !chunk.is_empty() {
            results.push(PathBuf::from(std::ffi::OsStr::from_bytes(chunk)));
        }
    }

    Ok(results)
}
