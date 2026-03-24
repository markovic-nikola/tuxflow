use tuxflow::mcp::bridge::LogBuffer;

#[test]
fn push_and_recent() {
    let mut buf = LogBuffer::new();
    buf.push("line 1".to_string());
    buf.push("line 2".to_string());
    buf.push("line 3".to_string());

    assert_eq!(buf.recent(3), vec!["line 1", "line 2", "line 3"]);
}

#[test]
fn recent_fewer_than_available() {
    let mut buf = LogBuffer::new();
    buf.push("a".to_string());
    buf.push("b".to_string());
    buf.push("c".to_string());

    assert_eq!(buf.recent(2), vec!["b", "c"]);
}

#[test]
fn recent_more_than_available() {
    let mut buf = LogBuffer::new();
    buf.push("only".to_string());

    assert_eq!(buf.recent(100), vec!["only"]);
}

#[test]
fn recent_empty_buffer() {
    let buf = LogBuffer::new();
    assert!(buf.recent(10).is_empty());
}

#[test]
fn evicts_oldest_at_capacity() {
    let mut buf = LogBuffer::new();
    // Push 1001 lines — buffer max is 1000
    for i in 0..1001 {
        buf.push(format!("line {i}"));
    }

    let recent = buf.recent(1000);
    assert_eq!(recent.len(), 1000);
    // First line should have been evicted
    assert_eq!(recent[0], "line 1");
    assert_eq!(recent[999], "line 1000");
}

#[test]
fn recent_preserves_order() {
    let mut buf = LogBuffer::new();
    for i in 0..10 {
        buf.push(format!("{i}"));
    }

    let recent = buf.recent(5);
    assert_eq!(recent, vec!["5", "6", "7", "8", "9"]);
}
