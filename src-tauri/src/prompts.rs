use crate::evidence::{CmdKind, DiscoveredCommand};
use crate::store::Criterion;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt::Write;

/// Build the planner prompt. The planner must answer with a JSON array of
/// acceptance criteria whose `cmd` values come verbatim from the discovered
/// command list.
pub fn planner_prompt(title: &str, intent: &str, commands: &[DiscoveredCommand]) -> String {
    let mut prompt = format!(
        "You are the planning agent for Temli. Turn the task below into executable \
         acceptance criteria grounded in commands that actually exist in the repository.\n\
         \n\
         TASK TITLE\n\
         {title}\n\
         \n\
         TASK INTENT\n\
         {intent}\n\
         \n\
         DISCOVERED COMMANDS\n\
         The `cmd` field of every criterion MUST be copied verbatim from this list. \
         Never invent, rewrite, merge, or abbreviate commands.\n"
    );

    if commands.is_empty() {
        prompt.push_str("(no commands discovered)\n");
    } else {
        for (index, command) in commands.iter().enumerate() {
            let _ = writeln!(
                &mut prompt,
                "{}. cmd: {}\n   kind: {}\n   source: {}",
                index + 1,
                command.cmd,
                kind_label(&command.kind),
                command.source
            );
        }
    }

    prompt.push_str(
        "\n\
         OUTPUT CONTRACT\n\
         Return ONLY a JSON array. Do not output prose, explanations, headings, \
         markdown fences, or any text before or after the array.\n\
         Each array element MUST be an object with exactly these fields:\n\
         - \"id\": unique stable id, sequential from \"c-1\" (\"c-1\", \"c-2\", ...).\n\
         - \"cmd\": one of the discovered commands above, exactly as written.\n\
         - \"expect_exit\": expected process exit code as an integer; normally 0.\n\
         - \"expect_match\": a text fragment that must appear in the command output, \
         or null when the output does not need an assertion.\n\
         \n\
         Example:\n\
         [{\"id\":\"c-1\",\"cmd\":\"cargo test\",\"expect_exit\":0,\"expect_match\":\"test result: ok\"}]\n\
         \n\
         RULES\n\
         - Produce at least one criterion and never an empty array.\n\
         - IDs MUST be unique across the array.\n\
         - Prefer criteria that prove the task through tests, then build and lint commands when relevant.\n\
         - Every `cmd` MUST appear verbatim in DISCOVERED COMMANDS.\n\
         - Do not add fields beyond the four listed above.\n",
    );

    prompt
}

/// Parse planner output into acceptance criteria. Tolerates markdown fences and
/// surrounding prose by extracting the first balanced JSON array.
pub fn parse_criteria(output: &str) -> Result<Vec<Criterion>, String> {
    let array = extract_balanced(output, '[', ']')?;
    let raw: Vec<RawCriterion> =
        serde_json::from_str(array).map_err(|error| format!("验收标准 JSON 解析失败：{error}"))?;

    if raw.is_empty() {
        return Err("验收标准数组不能为空".to_string());
    }

    let mut seen = HashSet::new();
    let mut criteria = Vec::with_capacity(raw.len());
    for (index, item) in raw.into_iter().enumerate() {
        let position = index + 1;
        let id = item.id.trim().to_string();
        if id.is_empty() {
            return Err(format!("第 {position} 条验收标准缺少 id"));
        }
        if !seen.insert(id.clone()) {
            return Err(format!("验收标准 id 重复：{id}"));
        }
        if item.cmd.trim().is_empty() {
            return Err(format!("验收标准 {id} 缺少 cmd"));
        }

        criteria.push(Criterion {
            id,
            cmd: item.cmd,
            expect_exit: item.expect_exit.unwrap_or(0),
            expect_match: item.expect_match,
        });
    }

    Ok(criteria)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewVerdict {
    pub verdict: String,
    pub summary: String,
    pub risk_flags: Vec<String>,
    /// Test/CI files the reviewer inspected, as reported by the reviewer.
    pub test_diff: Vec<String>,
}

/// Build the independent reviewer prompt. The reviewer must answer with a
/// single JSON object and explicitly call out test/CI changes.
pub fn reviewer_prompt(title: &str, diff: &str, evidence: &str, test_changed: &[String]) -> String {
    let mut prompt = format!(
        "You are the independent reviewer for Temli. Review the change against the task \
         and the collected evidence. Do not trust the implementing agent's self-report; \
         judge the diff and evidence yourself.\n\
         \n\
         TASK TITLE\n\
         {title}\n\
         \n\
         CHANGED TEST/CI FILES\n"
    );

    if test_changed.is_empty() {
        prompt.push_str("(none detected)\n");
    } else {
        for path in test_changed {
            let _ = writeln!(&mut prompt, "- {path}");
        }
    }

    let _ = write!(
        &mut prompt,
        "\n\
         DIFF\n\
         --- BEGIN DIFF ---\n\
         {diff}\n\
         --- END DIFF ---\n\
         \n\
         EVIDENCE\n\
         --- BEGIN EVIDENCE ---\n\
         {evidence}\n\
         --- END EVIDENCE ---\n"
    );

    prompt.push_str(
        "\n\
         REVIEW RULES\n\
         - \"pass\": the diff implements the task and the evidence supports it.\n\
         - \"fail\": the change is incorrect, incomplete, or unsafe.\n\
         - \"needs_human\": the change cannot be verified confidently, for example test or CI \
         assertions changed without a trustworthy before/after comparison, or a high-risk change \
         needs a human decision.\n\
         - Call out every change to tests or CI. If CHANGED TEST/CI FILES is not empty, or the \
         diff touches test or CI files, enumerate each one in \"summary\" and add a matching entry \
         to \"risk_flags\" (for example \"tests changed: tests/api.rs\" or \
         \"ci changed: .github/workflows/ci.yml\").\n\
         - Also list other risks in \"risk_flags\": deleted files, dependency changes, migrations, \
         permissions, secrets. Use an empty array when there are none.\n\
         - Enumerate every test or CI file you actually inspected in \"test_diff\", using the \
         repository-relative path. Do not list files you did not look at.\n\
         \n\
         OUTPUT CONTRACT\n\
         Return ONLY one JSON object. No prose, no markdown fences, no text before or after it.\n\
         {\"verdict\":\"pass|fail|needs_human\",\"summary\":\"...\",\"risk_flags\":[\"...\"],\"test_diff\":[\"...\"]}\n\
         - \"verdict\" MUST be exactly one of \"pass\", \"fail\", or \"needs_human\".\n\
         - \"summary\" MUST be a concise review summary.\n\
         - \"risk_flags\" MUST be an array of strings (possibly empty).\n\
         - \"test_diff\" MUST be an array of strings (possibly empty) listing every test or CI \
         file inspected, for example [\"tests/api.rs\", \".github/workflows/ci.yml\"].\n",
    );

    prompt
}

/// Parse reviewer output. Tolerates fences and prose by extracting the first
/// balanced JSON object, validates the verdict, and defaults optional fields.
pub fn parse_review(output: &str) -> Result<ReviewVerdict, String> {
    let object = extract_balanced(output, '{', '}')?;
    let raw: RawReview =
        serde_json::from_str(object).map_err(|error| format!("审查 JSON 解析失败：{error}"))?;

    let verdict = raw.verdict.as_deref().map(str::trim).unwrap_or("");
    let verdict = match verdict {
        "pass" | "fail" | "needs_human" => verdict.to_string(),
        "" => return Err("审查结果缺少 verdict".to_string()),
        other => {
            return Err(format!(
                "未知的审查结论：{other}（允许 pass、fail、needs_human）"
            ));
        }
    };

    Ok(ReviewVerdict {
        verdict,
        summary: raw.summary.unwrap_or_default(),
        risk_flags: raw.risk_flags.unwrap_or_default(),
        test_diff: raw.test_diff,
    })
}

#[derive(Deserialize)]
struct RawCriterion {
    #[serde(default)]
    id: String,
    #[serde(default)]
    cmd: String,
    #[serde(default)]
    expect_exit: Option<i32>,
    #[serde(default)]
    expect_match: Option<String>,
}

#[derive(Deserialize)]
struct RawReview {
    #[serde(default)]
    verdict: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    risk_flags: Option<Vec<String>>,
    #[serde(default)]
    test_diff: Vec<String>,
}

fn kind_label(kind: &CmdKind) -> &'static str {
    match kind {
        CmdKind::Test => "test",
        CmdKind::Build => "build",
        CmdKind::Lint => "lint",
    }
}

/// Extract the first balanced `open`..`close` span, ignoring delimiters that
/// appear inside JSON strings. Prose and markdown fences around it are ignored.
fn extract_balanced(output: &str, open: char, close: char) -> Result<&str, String> {
    let start = output
        .find(open)
        .ok_or_else(|| format!("输出中没有找到以 {open} 开头的 JSON 内容"))?;

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in output[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
        } else if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Ok(&output[start..start + offset + ch.len_utf8()]);
            }
        }
    }

    Err(format!("JSON 结构不完整：缺少与 {open} 配对的 {close}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered(cmd: &str, kind: CmdKind, source: &str) -> DiscoveredCommand {
        DiscoveredCommand {
            cmd: cmd.to_string(),
            kind,
            source: source.to_string(),
        }
    }

    #[test]
    fn planner_prompt_lists_every_discovered_command() {
        let commands = vec![
            discovered("cargo test", CmdKind::Test, "Cargo.toml"),
            discovered("cargo build", CmdKind::Build, "Cargo.toml"),
            discovered("npm run lint", CmdKind::Lint, "package.json"),
        ];

        let prompt = planner_prompt("Add health check", "Expose GET /health", &commands);

        assert!(prompt.contains("Add health check"));
        assert!(prompt.contains("Expose GET /health"));
        for command in &commands {
            assert!(
                prompt.contains(command.cmd.as_str()),
                "prompt is missing command {}",
                command.cmd
            );
            assert!(
                prompt.contains(command.source.as_str()),
                "prompt is missing source {}",
                command.source
            );
            assert!(prompt.contains(kind_label(&command.kind)));
        }
        assert!(prompt.contains("c-1"));
        assert!(prompt.contains("\"expect_exit\""));
        assert!(prompt.contains("\"expect_match\""));
        assert!(prompt.contains("Return ONLY a JSON array"));
        assert!(prompt.contains("verbatim"));
    }

    #[test]
    fn parse_criteria_accepts_fenced_json_with_prose() {
        let output = "Here is the plan:\n\
                      ```json\n\
                      [\n\
                        {\"id\":\"c-1\",\"cmd\":\"cargo test\",\"expect_exit\":0,\"expect_match\":\"test result: ok\"},\n\
                        {\"id\":\"c-2\",\"cmd\":\"cargo build\",\"expect_exit\":0,\"expect_match\":null}\n\
                      ]\n\
                      ```\n\
                      That completes the plan.";

        let criteria = parse_criteria(output).expect("fenced criteria should parse");

        assert_eq!(criteria.len(), 2);
        assert_eq!(criteria[0].id, "c-1");
        assert_eq!(criteria[0].cmd, "cargo test");
        assert_eq!(criteria[0].expect_exit, 0);
        assert_eq!(criteria[0].expect_match.as_deref(), Some("test result: ok"));
        assert_eq!(criteria[1].id, "c-2");
        assert_eq!(criteria[1].cmd, "cargo build");
        assert!(criteria[1].expect_match.is_none());
    }

    #[test]
    fn parse_criteria_accepts_leading_prose() {
        let output = "I checked package.json and Cargo.toml.\n\
                      [{\"id\":\"c-1\",\"cmd\":\"cargo test\",\"expect_exit\":1}]";

        let criteria = parse_criteria(output).expect("leading prose should be tolerated");

        assert_eq!(criteria.len(), 1);
        assert_eq!(criteria[0].expect_exit, 1);
    }

    #[test]
    fn parse_criteria_defaults_missing_expect_exit() {
        let criteria = parse_criteria(r#"[{"id":"c-1","cmd":"cargo test"}]"#)
            .expect("criteria without expect_exit should parse");

        assert_eq!(criteria[0].expect_exit, 0);
        assert!(criteria[0].expect_match.is_none());
    }

    #[test]
    fn parse_criteria_rejects_duplicate_ids() {
        let output = r#"[{"id":"c-1","cmd":"cargo test"},{"id":"c-1","cmd":"cargo build"}]"#;
        let error = parse_criteria(output).expect_err("duplicate ids must be rejected");

        assert!(error.contains("c-1"), "unexpected error: {error}");
    }

    #[test]
    fn parse_criteria_rejects_empty_array() {
        assert!(parse_criteria("[]").is_err());
        assert!(parse_criteria("```json\n[]\n```").is_err());
    }

    #[test]
    fn parse_criteria_rejects_invalid_output() {
        assert!(parse_criteria("there is no array here").is_err());
        assert!(parse_criteria(r#"[{"id":"c-1","cmd":}]"#).is_err());
        assert!(parse_criteria(r#"[{"id":" ","cmd":"cargo test"}]"#).is_err());
        assert!(parse_criteria(r#"[{"id":"c-1","cmd":""}]"#).is_err());
        assert!(parse_criteria(r#"{"criteria":[]}"#).is_err());
    }

    #[test]
    fn reviewer_prompt_includes_context_and_contract() {
        let changed = vec![
            "tests/health.rs".to_string(),
            ".github/workflows/ci.yml".to_string(),
        ];

        let prompt = reviewer_prompt(
            "Add health check",
            "diff --git a/src/health.rs b/src/health.rs",
            "cargo test: 6 passed",
            &changed,
        );

        assert!(prompt.contains("Add health check"));
        assert!(prompt.contains("diff --git a/src/health.rs"));
        assert!(prompt.contains("cargo test: 6 passed"));
        for path in &changed {
            assert!(prompt.contains(path.as_str()));
        }
        assert!(prompt.contains("\"verdict\":\"pass|fail|needs_human\""));
        assert!(prompt.contains("\"risk_flags\""));
        assert!(prompt.contains("\"test_diff\""));
        assert!(prompt.contains("TEST/CI"));
        assert!(prompt.contains("tests or CI"));
        assert!(prompt.contains("file inspected"));
    }

    #[test]
    fn parse_review_accepts_allowed_verdicts() {
        for verdict in ["pass", "fail", "needs_human"] {
            let output = format!(
                "Review:\n```json\n{{\"verdict\":\"{verdict}\",\"summary\":\"summary for {verdict}\",\"risk_flags\":[\"tests changed: a.rs\"],\"test_diff\":[\"tests/a.rs\"]}}\n```"
            );

            let review = parse_review(&output).expect("allowed verdict should parse");

            assert_eq!(review.verdict, verdict);
            assert_eq!(review.summary, format!("summary for {verdict}"));
            assert_eq!(review.risk_flags, vec!["tests changed: a.rs".to_string()]);
            assert_eq!(review.test_diff, vec!["tests/a.rs".to_string()]);
        }
    }

    #[test]
    fn parse_review_rejects_unknown_verdict() {
        let error = parse_review(r#"{"verdict":"lgtm","summary":"looks fine"}"#)
            .expect_err("unknown verdict must be rejected");

        assert!(error.contains("lgtm"), "unexpected error: {error}");
    }

    #[test]
    fn parse_review_defaults_summary_risk_flags_and_test_diff() {
        let output = "```json\n{\"verdict\":\"pass\"}\n```";
        let review = parse_review(output).expect("missing optional fields should parse");

        assert_eq!(review.verdict, "pass");
        assert_eq!(review.summary, "");
        assert!(review.risk_flags.is_empty());
        assert!(review.test_diff.is_empty());
    }

    #[test]
    fn parse_review_requires_valid_object() {
        assert!(parse_review("no object here").is_err());
        assert!(parse_review(r#"{"summary":"missing verdict"}"#).is_err());
        assert!(parse_review(r#"{"verdict":"pass""#).is_err());
        assert!(parse_review(r#"{"verdict":"pass","summary":"ok","risk_flags":"x"}"#).is_err());
        assert!(parse_review(r#"{"verdict":"pass","test_diff":"x"}"#).is_err());
    }
}
