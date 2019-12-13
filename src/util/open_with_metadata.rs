use std::fs::{Metadata, OpenOptions as StdOpenOptions};
use std::io::Error as IoError;
use std::path::Path;
use tokio::fs::{File, OpenOptions};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

/// Open a file and get metadata.
pub async fn open_with_metadata(path: impl AsRef<Path>) -> Result<(File, Metadata), IoError> {
    let mut opts = StdOpenOptions::new();
    opts.read(true);

    // On Windows, we need to set this flag to be able to open directories.
    #[cfg(windows)]
    opts.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);

    let file = OpenOptions::from(opts).open(path).await?;
    let metadata = file.metadata().await?;
    Ok((file, metadata))
}
