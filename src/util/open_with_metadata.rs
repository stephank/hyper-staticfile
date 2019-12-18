use std::fs::{File, Metadata, OpenOptions};
use std::io::Error as IoError;
use std::path::Path;
use tokio::task::spawn_blocking;

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

/// Open a file and get metadata.
pub async fn open_with_metadata(path: impl AsRef<Path>) -> Result<(File, Metadata), IoError> {
    let path = path.as_ref().to_owned();
    spawn_blocking(move || {
        let mut opts = OpenOptions::new();
        opts.read(true);

        // On Windows, we need to set this flag to be able to open directories.
        #[cfg(windows)]
        opts.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);

        let file = opts.open(path)?;
        let metadata = file.metadata()?;
        Ok((file, metadata))
    }).await?
}
