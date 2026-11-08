use super::*;
use std::{cell::Cell, time::Duration};

#[test]
fn partial_frame_progress_cannot_renew_the_original_read_deadline() {
    let (reader, mut writer) = UnixStream::pair().unwrap();
    writer.write_all(&1000u32.to_be_bytes()).unwrap();
    writer.write_all(b"x").unwrap();
    let worker = std::thread::spawn(move || {
        for _ in 0..400 {
            std::thread::sleep(Duration::from_millis(5));
            if writer.write_all(b"x").is_err() {
                break;
            }
        }
    });
    let mut stream = Stream::new(reader, Instant::now() + Duration::from_millis(200)).unwrap();
    let error = super::super::read_frame::<serde_json::Value>(&mut stream).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    drop(stream);
    worker.join().unwrap();
}

#[test]
fn blocked_frame_write_has_the_same_absolute_budget() {
    let (writer, mut reader) = UnixStream::pair().unwrap();
    let size: libc::c_int = 4096;
    assert_eq!(
        unsafe {
            libc::setsockopt(
                writer.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&size as *const libc::c_int).cast(),
                std::mem::size_of_val(&size) as libc::socklen_t,
            )
        },
        0
    );
    let payload = "x".repeat(1024 * 1024);
    assert!(serde_json::to_vec(&payload).unwrap().len() < super::super::MAX_FRAME);
    let mut stream = Stream::new(writer, Instant::now() + Duration::from_millis(200)).unwrap();
    let error = super::super::write_frame(&mut stream, &payload).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    drop(stream);
    let mut partial = Vec::new();
    reader.read_to_end(&mut partial).unwrap();
    assert!(!partial.is_empty(), "the fixture must reach a partial socket write");
}

#[test]
fn expired_deadline_refuses_even_an_already_connected_stream() {
    let (stream, _peer) = UnixStream::pair().unwrap();
    assert_eq!(
        Stream::new(stream, Instant::now()).err().unwrap().kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn scoped_inspection_deadline_restores_ordinary_policy_on_error_and_unwind() {
    fn replace(target: &mut Option<Instant>, next: Option<Instant>) -> Option<Instant> {
        std::mem::replace(target, next)
    }
    let original = Some(Instant::now() + Duration::from_secs(20));
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut policy = original;
    let result = with_deadline_policy(&mut policy, deadline, replace, |current| {
        assert_eq!(*current, Some(deadline));
        Err::<(), _>("failed read")
    });
    assert!(result.is_err());
    assert_eq!(policy, original);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_deadline_policy(&mut policy, deadline, replace, |_| panic!("read panic"));
    }));
    assert!(panic.is_err());
    assert_eq!(policy, original);
}

#[test]
fn uncancellable_inspection_still_terminates_but_never_publishes_marker() {
    fn terminate(code: i32) -> ! {
        std::panic::panic_any(code)
    }
    for readonly in [true, false] {
        let writes = Cell::new(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cancel_failed(readonly, || writes.set(writes.get() + 1), terminate);
        }));
        assert_eq!(*result.unwrap_err().downcast::<i32>().unwrap(), 1);
        assert_eq!(writes.get(), if readonly { 0 } else { 1 });
    }
}
