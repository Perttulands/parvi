use parvi_mission_engine::{
    Claude, Codex, HarnessAdapter, Pi, Pointer, Shell, SlotBinding, TokenUsage, TranscriptRoots,
    TurnState,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/harness")
        .join(path)
}
fn pointer(name: &str, path: PathBuf) -> Pointer {
    Pointer {
        text: format!(
            "Read the file /work/briefs/{name}.md and execute it exactly; it is your whole brief."
        ),
        cwd: "/work".into(),
        binding: Some(
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("codex"))
            {
                SlotBinding {
                    model: "gpt-6-astra".into(),
                    effort: "low".into(),
                }
            } else {
                SlotBinding {
                    model: "opus".into(),
                    effort: "medium".into(),
                }
            },
        ),
        transcript: Some(path),
        ..Pointer::default()
    }
}
#[test]
fn operator_turn_fixtures_complete_only_after_native_end() {
    for (adapter, cases) in [
        (
            &Claude as &dyn HarnessAdapter,
            vec![
                ("claude-operator-queued-mid-tool", "midtool"),
                ("claude-operator-after-answer", "afteranswer"),
                ("claude-operator-chat-then-sentinel", "chat"),
                ("claude-background-then-operator", "midturn"),
            ],
        ),
        (
            &Codex as &dyn HarnessAdapter,
            vec![
                ("codex-operator-steers-mid-turn", "midturn"),
                ("codex-operator-after-answer", "afteranswer"),
                ("codex-operator-chat-then-sentinel", "chat"),
            ],
        ),
    ] {
        for (name, brief) in cases {
            let p = pointer(brief, fixture(&format!("seats/{name}.jsonl")));
            let result = adapter
                .turn(&TranscriptRoots::default(), &p, "run_fixture")
                .unwrap();
            assert!(
                matches!(result.state, TurnState::Completed { .. }),
                "{name}: {result:?}"
            );
            assert!(result.operator_turns > 0, "{name}");
        }
    }
}
#[test]
fn unfinished_and_foreign_sentinel_wait_and_model_switch_errors() {
    for (adapter, id) in [
        (&Claude as &dyn HarnessAdapter, "claude"),
        (&Codex as &dyn HarnessAdapter, "codex"),
    ] {
        for (name, brief) in [
            ("no-sentinel", "nosentinel"),
            ("other-run-sentinel", "otherrun"),
        ] {
            let p = pointer(brief, fixture(&format!("seats/{id}-{name}.jsonl")));
            assert_eq!(
                adapter
                    .turn(&TranscriptRoots::default(), &p, "run_fixture")
                    .unwrap()
                    .state,
                TurnState::EndedWithoutSentinel
            );
        }
        let p = pointer("model", fixture(&format!("seats/{id}-model-switch.jsonl")));
        assert!(
            adapter
                .turn(&TranscriptRoots::default(), &p, "run_fixture")
                .is_err()
        );
    }
}
#[test]
fn token_readers_count_the_recorded_transcripts() {
    let mut p = pointer("unused", fixture("tokens/claude-subagent-session.jsonl"));
    p.text =
        "Read the file /work/sa/brief.md and execute it exactly; it is your whole brief.".into();
    assert_eq!(
        Claude.tokens_since(&p).unwrap(),
        TokenUsage {
            input: 44,
            cache_write: 24988,
            cache_read: 74213,
            output: 574
        }
    );
    p.transcript = Some(fixture("tokens/codex-two-turns.jsonl"));
    p.text =
        "Read the file /work/cx2/brief.md and execute it exactly; it is your whole brief.".into();
    assert_eq!(
        Codex.tokens_since(&p).unwrap(),
        TokenUsage {
            input: 3834,
            cache_write: 0,
            cache_read: 47616,
            output: 66
        }
    );
}
#[test]
fn pi_native_stop_and_tokens() {
    let temp = std::env::temp_dir().join(format!("parvi-104-pi-test-{}", std::process::id()));
    fs::create_dir_all(&temp).unwrap();
    let path = temp.join("session.jsonl");
    let p = Pointer {
        text: "pointer".into(),
        cwd: "/work".into(),
        transcript: Some(path.clone()),
        binding: Some(SlotBinding::default()),
        ..Pointer::default()
    };
    let head = "{\"type\":\"session\",\"id\":\"s\",\"cwd\":\"/work\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"pointer\"}}\n";
    for stop in ["toolUse", "length", "error", "aborted", "stop"] {
        let body = format!(
            "{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"<<<PARVI-DONE run-id=run_1 status=ok>>>\"}}],\"stopReason\":\"{stop}\",\"usage\":{{\"input\":10,\"output\":20,\"cacheRead\":100,\"cacheWrite\":5}}}}}}\n"
        );
        fs::write(&path, format!("{head}{body}")).unwrap();
        let result = Pi.turn(&TranscriptRoots::default(), &p, "run_1").unwrap();
        assert_eq!(
            matches!(result.state, TurnState::Completed { .. }),
            stop == "stop"
        );
        assert_eq!(
            Pi.tokens_since(&p).unwrap(),
            TokenUsage {
                input: 10,
                output: 20,
                cache_read: 100,
                cache_write: 5
            }
        );
    }
    fs::remove_dir_all(temp).unwrap();
}
#[test]
fn launch_quotes_both_shells_and_trusts_only_this_launch() {
    let binding = SlotBinding {
        model: "model'with quote".into(),
        effort: "high".into(),
    };
    let roots = TranscriptRoots {
        pi: "/sessions".into(),
        ..TranscriptRoots::default()
    };
    for shell in [Shell::Posix, Shell::PowerShell] {
        for adapter in [&Claude as &dyn HarnessAdapter, &Codex, &Pi] {
            let cmd = adapter
                .launch(&binding, Path::new("/work/a'b"), shell, &roots)
                .unwrap()
                .command;
            assert!(cmd.contains("high"));
            assert!(cmd.contains("model"));
            if shell == Shell::PowerShell {
                assert!(cmd.contains("''"));
            } else {
                assert!(cmd.contains("'\\''"));
            }
        }
        assert!(
            Codex
                .launch(&binding, Path::new("/work/a'b"), shell, &roots)
                .unwrap()
                .command
                .contains("trust_level")
        );
        assert!(
            Pi.launch(&binding, Path::new("/work/a'b"), shell, &roots)
                .unwrap()
                .command
                .contains("--session-dir")
        );
    }
}

#[test]
fn real_native_sessions_match_archon_readers() {
    let prompt = "Reply exactly: <<<PARVI-DONE run-id=run_parity status=ok artifact=none>>>";
    let cases: [(&dyn HarnessAdapter, &str, TokenUsage); 3] = [
        (
            &Claude,
            "claude",
            TokenUsage {
                input: 2,
                cache_write: 2142,
                cache_read: 649,
                output: 34,
            },
        ),
        (
            &Codex,
            "codex",
            TokenUsage {
                input: 2455,
                cache_write: 0,
                cache_read: 12288,
                output: 22,
            },
        ),
        (
            &Pi,
            "pi",
            TokenUsage {
                input: 4,
                cache_write: 647,
                cache_read: 0,
                output: 34,
            },
        ),
    ];
    for (adapter, name, expected) in cases {
        let p = Pointer {
            text: prompt.into(),
            cwd: "/work".into(),
            transcript: Some(fixture(&format!("real/{name}.jsonl"))),
            binding: Some(match name {
                "codex" => SlotBinding {
                    model: "gpt-6.1-sol".into(),
                    effort: "low".into(),
                },
                "claude" => SlotBinding {
                    model: "opus".into(),
                    effort: "medium".into(),
                },
                _ => SlotBinding {
                    model: "anthropic/opus".into(),
                    effort: "medium".into(),
                },
            }),
            ..Pointer::default()
        };
        let turn = adapter
            .turn(&TranscriptRoots::default(), &p, "run_parity")
            .unwrap();
        assert_eq!(
            turn.state,
            TurnState::Completed {
                text: "<<<PARVI-DONE run-id=run_parity status=ok artifact=none>>>".into()
            },
            "{name}"
        );
        assert_eq!(adapter.tokens_since(&p).unwrap(), expected, "{name}");
    }
}

#[test]
fn completed_turn_must_match_the_selected_model_and_effort() {
    let mut p = Pointer {
        text: "Reply exactly: <<<PARVI-DONE run-id=run_parity status=ok artifact=none>>>".into(),
        cwd: "/work".into(),
        transcript: Some(fixture("real/codex.jsonl")),
        binding: Some(SlotBinding {
            model: "gpt-6.1-sol".into(),
            effort: "low".into(),
        }),
        ..Pointer::default()
    };
    assert!(matches!(
        Codex
            .turn(&TranscriptRoots::default(), &p, "run_parity")
            .unwrap()
            .state,
        TurnState::Completed { .. }
    ));
    p.binding.as_mut().unwrap().effort = "high".into();
    assert!(
        Codex
            .turn(&TranscriptRoots::default(), &p, "run_parity")
            .is_err()
    );
}

#[test]
fn growing_transcripts_never_complete_before_the_native_end() {
    let scratch = std::env::temp_dir().join(format!("parvi-104-growing-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();
    for (adapter, id) in [
        (&Claude as &dyn HarnessAdapter, "claude"),
        (&Codex as &dyn HarnessAdapter, "codex"),
    ] {
        let fixture = fs::read_to_string(fixture(&format!(
            "seats/{id}-operator-chat-then-sentinel.jsonl"
        )))
        .unwrap();
        let path = scratch.join(format!("{id}.jsonl"));
        let p = pointer("chat", path.clone());
        let mut written = String::new();
        let mut completed_at = None;
        for (i, line) in fixture.lines().enumerate() {
            written.push_str(line);
            written.push('\n');
            fs::write(&path, &written).unwrap();
            let turn = adapter
                .turn(&TranscriptRoots::default(), &p, "run_fixture")
                .unwrap();
            if matches!(turn.state, TurnState::Completed { .. }) {
                completed_at = Some(i);
                break;
            }
        }
        let completion = completed_at.expect("eventually completed");
        let line = fixture.lines().nth(completion).unwrap();
        if id == "claude" {
            assert!(line.contains("<<<PARVI-DONE"));
        } else {
            assert!(line.contains("task_complete"));
        }
    }
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn claude_reports_background_work_until_native_notice() {
    let scratch = std::env::temp_dir().join(format!("parvi-104-background-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();
    let path = scratch.join("turn.jsonl");
    let source =
        fs::read_to_string(fixture("seats/claude-background-then-operator.jsonl")).unwrap();
    let prefix = source.lines().take(8).collect::<Vec<_>>().join("\n");
    fs::write(&path, &prefix).unwrap();
    let p = pointer("midturn", path.clone());
    assert_eq!(
        Claude
            .turn(&TranscriptRoots::default(), &p, "run_fixture")
            .unwrap()
            .state,
        TurnState::BackgroundWork
    );
    let notice = r#"{"type":"user","sessionId":"claude-fixture-midturn-session","message":{"content":"<task-notification><tool-use-id>toolu_fixture_3</tool-use-id><status>completed</status></task-notification>"},"isMeta":true}"#;
    // Preserve the fixture's actual session identity on the native notification.
    let first: serde_json::Value = serde_json::from_str(source.lines().next().unwrap()).unwrap();
    let notice = notice.replace(
        "claude-fixture-midturn-session",
        first["sessionId"].as_str().unwrap(),
    );
    fs::write(&path, format!("{prefix}\n{notice}\n")).unwrap();
    assert_eq!(
        Claude
            .turn(&TranscriptRoots::default(), &p, "run_fixture")
            .unwrap()
            .state,
        TurnState::EndedWithoutSentinel
    );
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn claude_only_consumes_a_human_pointer_in_the_dispatch_cwd() {
    let scratch = std::env::temp_dir().join(format!("parvi-104-pointer-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();
    let path = scratch.join("turn.jsonl");
    let pointer_text = "brief";
    let p = Pointer {
        text: pointer_text.into(),
        cwd: "/work".into(),
        transcript: Some(path.clone()),
        binding: Some(SlotBinding {
            model: "opus".into(),
            effort: "medium".into(),
        }),
        ..Pointer::default()
    };
    let wrapped = format!(
        "\n\n<pasted_content id=\"57da\">\n{pointer_text}\n</pasted_content id=\"57da\">\n"
    );
    for (content, cwd, session, origin, meta, want) in [
        (pointer_text, "/work", "session", "human", false, true),
        (wrapped.as_str(), "/work", "session", "human", false, true),
        ("brief plus more", "/work", "session", "human", false, false),
        (
            wrapped.as_str(),
            "/elsewhere",
            "session",
            "human",
            false,
            false,
        ),
        (wrapped.as_str(), "/work", "", "human", false, false),
        (wrapped.as_str(), "/work", "session", "task", false, false),
        (wrapped.as_str(), "/work", "session", "human", true, false),
    ] {
        let user = serde_json::json!({"type":"user","cwd":cwd,"sessionId":session,"origin":{"kind":origin},"isMeta":meta,"message":{"content":content}});
        let answer = serde_json::json!({"type":"assistant","cwd":"/work","sessionId":"session","message":{"model":"claude-opus-5","stop_reason":"end_turn","content":[{"type":"text","text":"<<<PARVI-DONE run-id=run_1 status=ok>>>"}]}});
        fs::write(&path, format!("{user}\n{answer}\n")).unwrap();
        let result = Claude
            .turn(&TranscriptRoots::default(), &p, "run_1")
            .unwrap();
        assert_eq!(
            result.consumed, want,
            "{content:?} {cwd} {session} {origin} {meta}"
        );
        assert_eq!(matches!(result.state, TurnState::Completed { .. }), want);
    }
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn claude_detects_replaced_conversation_and_pi_reports_unknown() {
    let scratch =
        std::env::temp_dir().join(format!("parvi-104-conversation-{}", std::process::id()));
    let sessions = scratch.join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let roots = TranscriptRoots {
        claude: scratch.join("projects"),
        ..TranscriptRoots::default()
    };
    let p = Pointer {
        pid: Some(4242),
        session_id: Some("old".into()),
        ..Pointer::default()
    };
    assert_eq!(Claude.conversation_replaced(&roots, &p).unwrap(), None);
    fs::write(sessions.join("4242.json"), r#"{"sessionId":"old"}"#).unwrap();
    assert_eq!(
        Claude.conversation_replaced(&roots, &p).unwrap(),
        Some(false)
    );
    fs::write(sessions.join("4242.json"), r#"{"sessionId":"new"}"#).unwrap();
    assert_eq!(
        Claude.conversation_replaced(&roots, &p).unwrap(),
        Some(true)
    );
    assert_eq!(Pi.conversation_replaced(&roots, &p).unwrap(), None);
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn transcript_discovery_skips_files_older_than_the_dispatch() {
    use std::time::{Duration, SystemTime};
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "parvi-105-transcripts-{}",
        u64::from_ne_bytes(nonce)
    ));
    fs::create_dir_all(&dir).unwrap();
    let stale = dir.join("old.jsonl");
    fs::write(&stale, "not a transcript\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(3600)),
        )
        .unwrap();
    // Written, not copied: a copy on Windows keeps the fixture's older time.
    fs::write(
        dir.join("new.jsonl"),
        fs::read(fixture("real/codex.jsonl")).unwrap(),
    )
    .unwrap();
    let pointer = Pointer {
        text: "Reply exactly: <<<PARVI-DONE run-id=run_parity status=ok artifact=none>>>".into(),
        cwd: "/work".into(),
        not_before: Some(SystemTime::now() - Duration::from_secs(10)),
        binding: Some(SlotBinding {
            model: "gpt-6.1-sol".into(),
            effort: "low".into(),
        }),
        ..Default::default()
    };
    let roots = TranscriptRoots {
        codex: dir.clone(),
        ..Default::default()
    };
    assert!(matches!(
        Codex.turn(&roots, &pointer, "run_parity").unwrap().state,
        TurnState::Completed { .. }
    ));
    fs::remove_dir_all(dir).unwrap();
}
