//! The words Missions shows, as Archon's cockpit says them: run states,
//! harness names, staffing sentences, times and spans.

use parvi_missions_contract::{RunOutput, RunProjection as Run, Slot};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// A run reference names what the CLI can address and where it runs.
pub fn run_reference(run: &Run, device: &str) -> String {
    format!(
        "run {} (mission {}, {}) on {device}",
        run.run_id, run.mission_slug, run.status
    )
}
/// A gate reference includes the request sequence that makes the answer exact.
pub fn gate_reference(
    run: &Run,
    gate: &parvi_missions_contract::WaitingGate,
    device: &str,
) -> String {
    let what = if gate.pick.is_some() { "pick" } else { "gate" };
    format!(
        "{what} {} of run {} (mission {}, requested-seq {}) on {device}",
        gate.gate_id, run.run_id, run.mission_slug, gate.requested_seq
    )
}

/// A run state in the navigator: a word, never a dot.
pub fn state_word(status: &str) -> &str {
    match status {
        "waiting_human" => "your answer",
        other => other,
    }
}

/// A run state on the run bar's chip.
pub fn state_chip(status: &str) -> String {
    match status {
        "waiting_human" => "WAITING FOR YOUR ANSWER".into(),
        other => other.replace('_', " ").to_uppercase(),
    }
}

/// The run needs the operator: it waits at a human gate or is blocked.
pub fn needs_you(run: &Run) -> bool {
    !run.r#final && (run.status == "waiting_human" || run.status == "blocked")
}

/// A harness as people call it.
pub fn harness_name(harness: &str) -> String {
    match harness {
        "claude-code" | "claude" => "Claude".into(),
        "openai-codex" | "codex" => "Codex".into(),
        "pi" => "Pi".into(),
        "" => "no harness".into(),
        other => other.into(),
    }
}

/// What works a slot, the same wherever it is shown: `Claude haiku · low`,
/// `Codex · medium` when the harness's own model is used.
pub fn slot_line(s: &Slot) -> String {
    let harness = harness_name(&s.harness);
    let mut parts = vec![if s.model.is_empty() || s.model == "default" {
        harness
    } else {
        format!("{harness} {}", s.model)
    }];
    if !s.effort.is_empty() {
        parts.push(s.effort.clone());
    }
    parts.join(" · ")
}

/// Who works a step: its role, then what works it, `Builder · Claude haiku
/// · low`.
pub fn who(s: &Slot) -> String {
    format!("{} · {}", role_name(&s.agent_id), slot_line(s))
}

/// A role id as its card's name: `planner` reads Planner.
pub fn role_name(agent: &str) -> String {
    if agent.is_empty() {
        return "vanilla".into();
    }
    let mut c = agent.chars();
    c.next()
        .map(|f| {
            f.to_uppercase()
                .chain(c)
                .collect::<String>()
                .replace(['-', '_'], " ")
        })
        .unwrap_or_default()
}

/// A human-readable slot label. Internal IDs are not names.
pub fn slot_label(slot: &Slot) -> &str {
    if slot.label.trim().is_empty() {
        "Agent"
    } else {
        slot.label.trim()
    }
}

/// Flow's staffing sentence for one slot: `Plan (controller) is Planner on
/// Claude · xhigh.`
pub fn staffing(s: &Slot) -> String {
    let role = if s.controller { " (controller)" } else { "" };
    format!(
        "{}{role} is {} on {}.",
        slot_label(s),
        role_name(&s.agent_id),
        slot_line(s)
    )
}

pub fn parse_time(iso: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(iso, &Rfc3339).ok()
}

fn local(at: OffsetDateTime) -> OffsetDateTime {
    time::UtcOffset::local_offset_at(at)
        .map(|o| at.to_offset(o))
        .unwrap_or(at)
}

/// A local time: `14:02` today, otherwise `1 Oct 14:02`.
pub fn short_time(iso: &str, now: OffsetDateTime) -> String {
    let Some(at) = parse_time(iso).map(local) else {
        return String::new();
    };
    let now = local(now);
    let clock = format!("{:02}:{:02}", at.hour(), at.minute());
    if at.date() == now.date() {
        return clock;
    }
    let month = &format!("{:?}", at.month())[..3];
    if at.year() == now.year() {
        format!("{} {month} {clock}", at.day())
    } else {
        format!("{} {month} {} {clock}", at.day(), at.year())
    }
}

/// A span in its two largest units: 45s, 3m 12s, 2h 5m, 3d 4h.
pub fn span(seconds: i64) -> String {
    let t = seconds.max(0);
    let (d, h, m, s) = (t / 86400, t % 86400 / 3600, t % 3600 / 60, t % 60);
    let two = |a: i64, ua: &str, b: i64, ub: &str| {
        if b > 0 {
            format!("{a}{ua} {b}{ub}")
        } else {
            format!("{a}{ua}")
        }
    };
    if d > 0 {
        two(d, "d", h, "h")
    } else if h > 0 {
        two(h, "h", m, "m")
    } else if m > 0 {
        two(m, "m", s, "s")
    } else {
        format!("{s}s")
    }
}

/// A limit's time exactly, as Archon writes durations: `45s`, `30m`,
/// `1h30m`, `1h0m5s`.
pub fn duration_text(seconds: i64) -> String {
    let t = seconds.max(0);
    let (h, m, s) = (t / 3600, t % 3600 / 60, t % 60);
    let mut out = String::new();
    if h > 0 {
        out += &format!("{h}h");
    }
    if m > 0 || (h > 0 && s > 0) {
        out += &format!("{m}m");
    }
    if s > 0 || out.is_empty() {
        out += &format!("{s}s");
    }
    out
}

/// A typed duration in whole seconds: `45s`, `30m`, `1h30m`, or a bare
/// number of minutes. Blank is none.
pub fn parse_duration(text: &str) -> Result<Option<i64>, String> {
    let t = text.trim().to_ascii_lowercase();
    if t.is_empty() {
        return Ok(None);
    }
    let bad = || format!("{text:?} is not a time: write 45s, 30m or 1h30m.");
    if let Ok(minutes) = t.parse::<i64>() {
        return if minutes < 0 {
            Err(bad())
        } else {
            Ok(Some(minutes * 60))
        };
    }
    let (mut total, mut digits) = (0i64, String::new());
    for c in t.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit = match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(bad()),
        };
        let n: i64 = digits.parse().map_err(|_| bad())?;
        total += n * unit;
        digits.clear();
    }
    if !digits.is_empty() {
        return Err(bad());
    }
    Ok(Some(total))
}

/// How long a run took, or has run so far.
pub fn run_span(run: &Run, now: OffsetDateTime) -> String {
    let Some(start) = parse_time(&run.started_at) else {
        return String::new();
    };
    let end = if run.r#final {
        parse_time(&run.updated_at).unwrap_or(now)
    } else {
        now
    };
    span((end - start).whole_seconds())
}

/// The start of what the run was given, on one line.
pub fn input_excerpt(run: &Run) -> String {
    let first = run
        .inputs
        .iter()
        .find(|i| i.kind == "text")
        .or(run.inputs.first());
    let Some(i) = first else {
        return String::new();
    };
    if matches!(i.kind.as_str(), "file" | "folder") {
        return i
            .value
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&i.value)
            .to_owned();
    }
    let text = i.value.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 90 {
        format!("{}…", text.chars().take(89).collect::<String>())
    } else {
        text
    }
}

/// A work reference as people write it: `gh:o/r#4` reads `o/r#4`, a Bead
/// `bead:node:/srv#x-1` reads `x-1`.
pub fn work_label(work: &str) -> String {
    if let Some(rest) = work.strip_prefix("gh:") {
        return rest.to_string();
    }
    if work.starts_with("bead:")
        && let Some((_, id)) = work.rsplit_once('#')
    {
        return id.to_string();
    }
    work.to_string()
}

/// Who an actor recorded in the ledger is, in the operator's words.
pub fn actor_label(actor: &str) -> String {
    match actor {
        "human:operator" => "you".into(),
        "parvi" | "archond" => "Parvi".into(),
        other => other.trim_start_matches("agent:").to_string(),
    }
}

/// An output as its chip shows it (Archon's runOutputValueLabel): a path's
/// last name, or the start of the text on one line.
pub fn output_excerpt(o: &RunOutput) -> String {
    const EXCERPT: usize = 40;
    if o.kind != "text" {
        let name = o
            .value
            .rsplit('/')
            .find(|p| !p.is_empty())
            .unwrap_or(&o.value);
        return if o.kind == "folder" {
            format!("{name}/")
        } else {
            name.to_string()
        };
    }
    let text = o.value.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > EXCERPT {
        format!("{}…", text.chars().take(EXCERPT - 1).collect::<String>())
    } else {
        text
    }
}

/// The branch a repository run works on, kept until it is landed.
pub fn run_branch(run: &Run) -> Option<String> {
    (!run.repository.is_empty()).then(|| format!("parvi/run-{}", run.run_id))
}

/// A file's size in a few characters: `512 B`, `2 KB`, `1.18 GB`.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024. && unit < UNITS.len() - 1 {
        v /= 1024.;
        unit += 1;
    }
    if unit >= 3 {
        format!("{v:.2} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

/// The most of a path a line shows: its start and its end, the middle cut,
/// so the root and the file's own name both stay readable.
pub fn middle(path: &str) -> String {
    const SHOWN: usize = 72;
    const HEAD: usize = 24;
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= SHOWN {
        return path.to_owned();
    }
    let tail = SHOWN - HEAD - 1;
    let head: String = chars[..HEAD].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{head}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_and_its_gate_are_handed_over_by_what_answers_them() {
        let run: Run = serde_json::from_value(serde_json::json!({"runId":"r-7","status":"waiting_human","final":false,"resumeAllowed":false,"epoch":1,"missionSlug":"daily","missionId":"m","missionRev":1,"inputCardId":"i","work":"","cwd":"","repository":"","base":"","contextPaths":[],"inputs":[],"outputs":[],"eventCount":1,"startedBy":"","startedAt":"","updatedAt":"","waitingGates":[],"events":[]})).unwrap();
        let gate = parvi_missions_contract::WaitingGate {
            gate_id: "gate_review".into(),
            requested_seq: 12,
            requested_at: String::new(),
            pick: None,
        };
        assert_eq!(
            run_reference(&run, "rig"),
            "run r-7 (mission daily, waiting_human) on rig"
        );
        assert_eq!(
            gate_reference(&run, &gate, "rig"),
            "gate gate_review of run r-7 (mission daily, requested-seq 12) on rig"
        );
    }
    #[test]
    fn an_output_chip_shows_a_path_name_or_the_start_of_its_text() {
        let out = |kind: &str, value: &str| RunOutput {
            name: "report".into(),
            kind: kind.into(),
            value: value.into(),
            node_id: String::new(),
            port_id: String::new(),
            attempt: 1,
            picks: vec![],
        };
        assert_eq!(
            output_excerpt(&out("file", "/w/run/report.md")),
            "report.md"
        );
        assert_eq!(output_excerpt(&out("folder", "/w/run/shots/")), "shots/");
        assert_eq!(
            output_excerpt(&out("text", "Ship it\n  now")),
            "Ship it now"
        );
        let long = "word ".repeat(20);
        let shown = output_excerpt(&out("text", &long));
        assert_eq!(shown.chars().count(), 40);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn a_limit_time_reads_back_exactly() {
        for secs in [45, 90, 1800, 5400, 3605, 3600] {
            let text = duration_text(secs);
            assert_eq!(parse_duration(&text), Ok(Some(secs)), "{text}");
        }
        assert_eq!(duration_text(90), "1m30s");
        assert_eq!(parse_duration("30"), Ok(Some(1800)));
        assert_eq!(parse_duration(" "), Ok(None));
        assert!(parse_duration("3x").is_err());
    }

    #[test]
    fn spans_and_labels() {
        assert_eq!(span(45), "45s");
        assert_eq!(span(192), "3m 12s");
        assert_eq!(span(7500), "2h 5m");
        assert_eq!(span(3 * 86400 + 4 * 3600), "3d 4h");
        assert_eq!(work_label("gh:acme/project#206"), "acme/project#206");
        assert_eq!(work_label("bead:ab12:/srv#srv-12"), "srv-12");
        assert_eq!(role_name("planner"), "Planner");
        let s = Slot {
            label: "Plan".into(),
            agent_id: "planner".into(),
            harness: "claude-code".into(),
            effort: "xhigh".into(),
            controller: true,
            ..Default::default()
        };
        assert_eq!(
            staffing(&s),
            "Plan (controller) is Planner on Claude · xhigh."
        );
    }

    #[test]
    fn times_read_local_and_short() {
        let now = parse_time("2026-11-08T21:31:00Z").unwrap();
        assert_eq!(short_time("2026-11-08T21:31:00Z", now).len(), 5);
        assert!(short_time("2025-01-02T10:00:00Z", now).contains("2025"));
        assert_eq!(short_time("nonsense", now), "");
    }

    #[test]
    fn a_long_path_keeps_its_root_and_its_name() {
        let path = format!("/tmp/{}/run_e29/haiku.txt", "deep/".repeat(30));
        let shown = middle(&path);
        assert_eq!(shown.chars().count(), 72);
        assert!(shown.starts_with("/tmp/deep/") && shown.ends_with("/run_e29/haiku.txt"));
        assert_eq!(middle("/srv/short"), "/srv/short");
    }
}
