use std::error::Error;
use std::io;
use std::thread;

use oryq_types::{
    CancellationSource, CancellationToken, ErrorCategory, ErrorEnvelope, ErrorLocation,
};

#[test]
fn error_envelope_preserves_metadata_location_and_source() {
    let io_err = io::Error::new(io::ErrorKind::UnexpectedEof, "truncated WAL frame");
    let env = ErrorEnvelope::new(
        ErrorCategory::Corruption,
        "engine",
        "wal::decode_frame",
        "checksum mismatch",
    )
    .with_location(ErrorLocation::new("app.oryq-wal", Some(42), Some(8256)))
    .with_source(io_err);

    assert_eq!(env.category(), ErrorCategory::Corruption);
    assert_eq!(env.module_id(), "engine");
    assert_eq!(env.operation(), "wal::decode_frame");
    assert!(env.source().is_some());
    let rendered = env.to_string();
    assert!(rendered.contains("[engine:wal::decode_frame:Corruption]"));
    assert!(rendered.contains("origin=app.oryq-wal"));
    assert!(rendered.contains("id=42"));
    assert!(rendered.contains("offset=8256"));
}

#[test]
fn cancellation_propagates_across_threads() {
    let never = CancellationToken::never();
    assert!(!never.is_cancelled());
    assert!(never.check("query", "scan").is_ok());

    let source = CancellationSource::new();
    let token = source.token();
    assert!(token.check("query", "scan").is_ok());

    let handle = thread::spawn(move || {
        while !token.is_cancelled() {
            thread::yield_now();
        }
        token.check("query", "scan")
    });

    source.cancel();
    let err = handle.join().unwrap().unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Cancelled);
    assert_eq!(err.module_id(), "query");
}
