#![allow(dead_code)]

include!("../src/main.rs");

#[test]
fn integration_preview_chat_mode_contains_turn_labels() {
    let dir = std::env::temp_dir().join(format!("cse-int-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("sess.jsonl");
    let data = [
        r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"x","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp/int"}}"#,
        r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"one"}]}}"#,
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"two"}]}}"#,
    ]
    .join("\n");
    std::fs::write(&path, data).expect("write");

    let s = SessionSummary {
        path,
        file_name: "sess.jsonl".to_string(),
        id: "x".to_string(),
        cwd: "/tmp/int".to_string(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
        event_count: 3,
        search_blob: "one two".to_string(),
    };

    let preview = build_preview(&s, PreviewMode::Chat, 90).expect("preview");
    let rendered = preview
        .lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("USER"));
    assert!(rendered.contains("ASSISTANT"));
}
