//! `.part` 文件的预留与发布。从 Python 的 `downloader.files` 模块迁移而来。
//!
//! 输出文件名通过 `.part` 文件在整个进程内进行抢占，使得两个并发
//! 下载永远不会指向同一路径；最终的文件会在移动到位时避免覆盖
//! 中途出现的同名文件。

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rdm_domain::DownloadTask;

/// 在所有下载工作线程之间串行化文件名选择，对应 Python 实现中的模块级锁。
static PATH_LOCK: Mutex<()> = Mutex::new(());

const MAX_ATTEMPTS: u32 = 10_000;

/// 输出路径对应的 `<name>.part` 兄弟路径。
fn part_sibling(output_path: &Path) -> PathBuf {
    let name = output_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    output_path.with_file_name(format!("{name}.part"))
}

/// 将请求的文件名拆分为 stem 与带点的后缀，例如
/// `"file.bin"` -> `("file", ".bin")`，`"archive.tar.gz"` -> `("archive.tar", ".gz")`。
fn stem_and_suffix(requested: &str) -> (String, String) {
    let path = Path::new(requested);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (stem, suffix)
}

fn candidate_name(requested: &str, stem: &str, suffix: &str, number: u32) -> String {
    if number == 0 {
        requested.to_string()
    } else {
        format!("{stem} ({number}){suffix}")
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    PATH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 通过 `.part` 文件在进程内原子地预留一个输出文件名，
/// 返回最终选定的文件名以及预留的（当前已存在、为空的）`.part` 路径。
pub fn reserve_part_file(destination: &Path, requested: &str) -> io::Result<(String, PathBuf)> {
    fs::create_dir_all(destination)?;
    let (stem, suffix) = stem_and_suffix(requested);

    let _guard = lock();
    for number in 0..MAX_ATTEMPTS {
        let name = candidate_name(requested, &stem, &suffix, number);
        let output_path = destination.join(&name);
        let part_path = part_sibling(&output_path);
        if output_path.exists() {
            continue;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
        // 输出文件可能在上述检查和占位 part 文件之间被竞态创建；
        // 丢弃我们的预留并尝试下一个名字。
        if output_path.exists() {
            let _ = fs::remove_file(&part_path);
            continue;
        }
        return Ok((name, part_path));
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not reserve an available output filename",
    ))
}

/// 将任务的 `.part` 文件发布为最终输出，且不会覆盖已有文件；
/// 若在后续循环中遇到名字冲突，会推进 `task.filename` 选用新的名字。
pub fn publish_part_file(task: &mut DownloadTask) -> io::Result<()> {
    let _guard = lock();
    let mut part_path = task.part_path();
    let requested = task.filename.clone();
    let (stem, suffix) = stem_and_suffix(&requested);
    let destination = PathBuf::from(&task.destination);

    for number in 0..MAX_ATTEMPTS {
        let name = candidate_name(&requested, &stem, &suffix, number);
        let output_path = destination.join(&name);
        let candidate_part = part_sibling(&output_path);
        if candidate_part != part_path {
            if output_path.exists() || candidate_part.exists() {
                continue;
            }
            fs::rename(&part_path, &candidate_part)?;
            part_path = candidate_part;
            task.filename = name;
        }
        // 用空占位符抢占输出名，以便检测此处出现的外部文件而非将其覆盖。
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
        if let Err(error) = fs::rename(&part_path, &output_path) {
            let _ = fs::remove_file(&output_path);
            return Err(error);
        }
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not publish to an available output filename",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(dir: &Path, filename: &str) -> DownloadTask {
        DownloadTask::create("https://example.test/file", dir, 8, filename, "").unwrap()
    }

    #[test]
    fn reserve_uses_next_available_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.bin"), b"existing").unwrap();

        let (name, part) = reserve_part_file(dir.path(), "file.bin").unwrap();

        assert_eq!(name, "file (1).bin");
        assert_eq!(
            part.file_name().unwrap().to_string_lossy(),
            "file (1).bin.part"
        );
        assert!(part.is_file());
    }

    #[test]
    fn reserve_skips_existing_part() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.bin.part"), b"partial").unwrap();

        let (name, part) = reserve_part_file(dir.path(), "file.bin").unwrap();

        assert_eq!(name, "file (1).bin");
        assert!(part.is_file());
    }

    #[test]
    fn reserve_handles_multi_dot_suffix() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("archive.tar.gz"), b"x").unwrap();

        let (name, _) = reserve_part_file(dir.path(), "archive.tar.gz").unwrap();

        assert_eq!(name, "archive.tar (1).gz");
    }

    #[test]
    fn publish_preserves_late_output_collision() {
        let dir = tempfile::tempdir().unwrap();
        let mut task = task(dir.path(), "file.bin");
        let (name, part) = reserve_part_file(dir.path(), &task.filename).unwrap();
        task.filename = name;
        fs::write(&part, b"downloaded").unwrap();
        // 一个无关文件在发布前落在了选定的输出名上。
        fs::write(task.output_path(), b"external").unwrap();

        publish_part_file(&mut task).unwrap();

        assert_eq!(fs::read(dir.path().join("file.bin")).unwrap(), b"external");
        assert_eq!(task.filename, "file (1).bin");
        assert_eq!(fs::read(task.output_path()).unwrap(), b"downloaded");
    }

    #[test]
    fn publish_moves_part_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut task = task(dir.path(), "file.bin");
        let (name, part) = reserve_part_file(dir.path(), &task.filename).unwrap();
        task.filename = name;
        fs::write(&part, b"payload").unwrap();

        publish_part_file(&mut task).unwrap();

        assert_eq!(task.filename, "file.bin");
        assert_eq!(fs::read(task.output_path()).unwrap(), b"payload");
        assert!(!part.exists());
    }
}
