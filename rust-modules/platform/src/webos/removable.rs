//! App-namespace observation only; no mount operation, root helper or UUID claim.
use crate::tv::removable::{candidates, MountCandidate, ReadError, INPUT_LIMIT};
use plx_base::task::OffFrame;
use std::io::Read;

pub fn observe(
    _off: &OffFrame,
    check: &mut dyn FnMut() -> bool,
) -> Result<Vec<MountCandidate>, ReadError> {
    if !check() {
        return Err(ReadError::Cancelled);
    }
    // This proc file describes this native process's namespace, including the
    // jail's mappings. Host platforms without proc report unavailable safely.
    let mut file =
        std::fs::File::open("/proc/self/mountinfo").map_err(|_| ReadError::Unavailable)?;
    read_snapshot(&mut file, check)
}

fn read_snapshot(
    reader: &mut dyn Read,
    check: &mut dyn FnMut() -> bool,
) -> Result<Vec<MountCandidate>, ReadError> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if !check() {
            return Err(ReadError::Cancelled);
        }
        let remaining = (INPUT_LIMIT + 1)
            .saturating_sub(bytes.len())
            .min(buffer.len());
        if remaining == 0 {
            return Err(ReadError::Invalid(
                crate::tv::removable::SnapshotError::TooLarge,
            ));
        }
        let count = reader
            .read(&mut buffer[..remaining])
            .map_err(|_| ReadError::Unavailable)?;
        if !check() {
            return Err(ReadError::Cancelled);
        }
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(ReadError::Invalid(
            crate::tv::removable::SnapshotError::InvalidRecord,
        ));
    }
    candidates(&bytes).map_err(ReadError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_error_and_truncation_are_not_an_empty_drive_list() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture"))
            }
        }
        assert_eq!(
            read_snapshot(&mut Broken, &mut || true),
            Err(ReadError::Unavailable)
        );
        assert!(matches!(
            read_snapshot(
                &mut &b"7 1 8:1 / /tmp/usb/a/b rw - ext4 /dev/a rw"[..],
                &mut || true
            ),
            Err(ReadError::Invalid(_))
        ));
    }
    #[test]
    fn cancellation_after_io_never_publishes_partial_snapshot() {
        let mut checks = 0;
        let mut check = || {
            checks += 1;
            checks < 2
        };
        let mut input = &b"7 1 8:1 / /tmp/usb/a/b rw - ext4 /dev/a rw\n"[..];
        assert_eq!(
            read_snapshot(&mut input, &mut check),
            Err(ReadError::Cancelled)
        );
    }
    #[test]
    fn size_limit_reads_only_one_overflow_byte_and_never_accepts_prefix() {
        let mut input = std::io::Cursor::new(vec![b' '; INPUT_LIMIT + 500]);
        assert!(matches!(
            read_snapshot(&mut input, &mut || true),
            Err(ReadError::Invalid(
                crate::tv::removable::SnapshotError::TooLarge
            ))
        ));
        assert_eq!(input.position(), (INPUT_LIMIT + 1) as u64);
    }
}
