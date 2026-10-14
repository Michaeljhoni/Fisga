use std::fs::File;
use std::io;

/// Writes the whole buffer at `offset` without moving a shared cursor, so several
/// connections can write to the same file concurrently.
pub(crate) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut written = 0;
        while written < buf.len() {
            let n = file.seek_write(&buf[written..], offset + written as u64)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            written += n;
        }
        Ok(())
    }
}
