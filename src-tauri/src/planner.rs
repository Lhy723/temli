//! 多任务规划：把一个想法变成带验收标准与依赖关系的任务草案。
//!
//! [`plan_prompt`] 约束规划器只能输出纯 JSON；[`parse_plan`] 在任何数据库写入前
//! 严格校验输出——任务数量、标题/提示、验收标准与依赖索引都必须合法，并且每条
//! 验收命令都要逐字出现在仓库实际发现的命令列表里，避免凭空生成不可执行的命令。

use crate::db::Project;
use crate::evidence::{CmdKind, DiscoveredCommand};
use crate::store::Criterion;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Write;

/// 单次规划允许的任务数量上限。
pub const MAX_PLAN_ISSUES: usize = 10;

/// 任务草案中的单个任务；确认后每个条目会变成一个 draft 任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIssue {
    pub title: String,
    pub prompt: String,
    pub acceptance: Vec<Criterion>,
    /// 前置任务在 `issues` 中的下标；只能引用更早的任务。
    pub depends_on: Vec<usize>,
}

/// 规划器输出的完整草案。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDraft {
    pub issues: Vec<PlanIssue>,
}

/// 构造规划器提示词。
///
/// 输出契约只允许一个 JSON 对象，并明确约束 1–10 个任务、非空且唯一的标题、
/// 每个任务至少一条验收标准、命令必须来自发现列表、依赖只能指向更早的任务。
pub fn plan_prompt(idea: &str, project: &Project, commands: &[DiscoveredCommand]) -> String {
    let mut prompt = format!(
        "You are the planning agent for Temli. Turn the idea below into a small, \
         dependency-ordered plan of implementation tasks for this project.\n\
         \n\
         PROJECT\n\
         name: {}\n\
         repository: {}\n\
         default branch: {}\n\
         \n\
         IDEA\n\
         {}\n\
         \n\
         DISCOVERED COMMANDS\n\
         The `cmd` field of every acceptance criterion MUST be copied verbatim from \
         this list. Never invent, rewrite, merge, or abbreviate commands.\n",
        project.name, project.repo_path, project.default_branch, idea
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
                kind_label(command.kind),
                command.source
            );
        }
    }

    prompt.push_str(
        "\n\
         OUTPUT CONTRACT\n\
         Return ONLY one JSON object, with exactly this shape:\n\
         {\"issues\":[{\"title\":\"short imperative title\",\"prompt\":\"self-contained implementation brief\",\"acceptance\":[{\"id\":\"c-1\",\"cmd\":\"exact discovered command\",\"expect_exit\":0,\"expect_match\":null}],\"depends_on\":[]}]}\n\
         Do not output prose, explanations, headings, markdown fences, comments, or any \
         text before or after the object.\n\
         \n\
         RULES\n\
         - Produce 1 to 10 issues. Never return an empty or oversized plan.\n\
         - `title` and `prompt` MUST be non-empty. Titles MUST be unique across issues.\n\
         - `title` is a short imperative name; `prompt` is a self-contained brief that \
           explains scope, expected behavior, and the tests to add or update.\n\
         - `acceptance` MUST contain at least one criterion per issue. Criterion `id` \
           values MUST be unique within an issue and use only letters, digits, `_`, or \
           `-` (for example \"c-1\", \"c-2\").\n\
         - `expect_exit` is the expected integer exit code (normally 0); `expect_match` \
           is a text fragment required in the command output, or null when no output \
           assertion is needed.\n\
         - Every acceptance `cmd` MUST appear verbatim in DISCOVERED COMMANDS.\n\
         - `depends_on` holds 0-based indices of EARLIER issues only. Use [] when an \
           issue has no prerequisites. Never reference itself or a later issue, and \
           order issues topologically so dependencies come first.\n\
         - Every issue must be independently implementable and verifiable, and together \
           the plan must cover the whole idea.\n\
         - Do not add fields beyond the ones shown in OUTPUT CONTRACT.\n",
    );

    prompt
}

/// 解析并校验规划器输出。
///
/// 容忍围栏与前后说明文字：抽取第一个平衡的 JSON 对象。抽取后仍要求严格符合
/// [`PlanDraft`] 结构，再交给 [`validate_plan`] 做语义校验。
pub fn parse_plan(output: &str, commands: &[DiscoveredCommand]) -> Result<PlanDraft, String> {
    let object = extract_json_object(output)?;
    let draft: PlanDraft =
        serde_json::from_str(object).map_err(|error| format!("计划 JSON 解析失败：{error}"))?;
    validate_plan(draft, commands)
}

/// 校验草案语义并返回规范化（去除标题/提示/验收 id 首尾空白）后的副本。
///
/// 该函数不接触数据库，供 [`parse_plan`] 与确认建任务的命令共用，保证任何写入
/// 之前计划都已经通过同样的严格检查。
pub fn validate_plan(
    mut draft: PlanDraft,
    commands: &[DiscoveredCommand],
) -> Result<PlanDraft, String> {
    if draft.issues.is_empty() {
        return Err("计划至少需要 1 个任务".to_string());
    }
    if draft.issues.len() > MAX_PLAN_ISSUES {
        return Err(format!(
            "计划最多允许 {MAX_PLAN_ISSUES} 个任务，实际为 {}",
            draft.issues.len()
        ));
    }

    let discovered: HashSet<&str> = commands
        .iter()
        .map(|command| command.cmd.as_str())
        .collect();
    let mut titles: HashSet<String> = HashSet::with_capacity(draft.issues.len());

    for (index, issue) in draft.issues.iter_mut().enumerate() {
        let position = index + 1;
        issue.title = issue.title.trim().to_string();
        issue.prompt = issue.prompt.trim().to_string();

        if issue.title.is_empty() {
            return Err(format!("第 {position} 个任务缺少标题"));
        }
        if issue.prompt.is_empty() {
            return Err(format!("任务「{}」缺少执行提示（prompt）", issue.title));
        }
        if !titles.insert(issue.title.clone()) {
            return Err(format!("任务标题重复：{}", issue.title));
        }

        if issue.acceptance.is_empty() {
            return Err(format!("任务「{}」至少需要 1 条验收标准", issue.title));
        }

        let title = issue.title.clone();
        let mut criterion_ids: HashSet<String> = HashSet::with_capacity(issue.acceptance.len());
        for (criterion_index, criterion) in issue.acceptance.iter_mut().enumerate() {
            criterion.id = criterion.id.trim().to_string();
            let criterion_position = criterion_index + 1;

            if criterion.id.is_empty() {
                return Err(format!(
                    "任务「{title}」第 {criterion_position} 条验收标准缺少 id"
                ));
            }
            if !is_valid_criterion_id(&criterion.id) {
                return Err(format!(
                    "任务「{title}」的验收标准 id 非法：{}（仅允许 A-Za-z0-9_-，长度 1-40）",
                    criterion.id
                ));
            }
            if !criterion_ids.insert(criterion.id.clone()) {
                return Err(format!(
                    "任务「{title}」的验收标准 id 重复：{}",
                    criterion.id
                ));
            }
            if criterion.cmd.trim().is_empty() {
                return Err(format!(
                    "任务「{title}」的验收标准 {} 缺少 cmd",
                    criterion.id
                ));
            }
            if !discovered.contains(criterion.cmd.as_str()) {
                return Err(format!(
                    "任务「{title}」的验收标准 {} 使用了未发现的命令：{}（命令必须逐字来自发现列表）",
                    criterion.id, criterion.cmd
                ));
            }
        }

        for (dependency_index, &depends_on) in issue.depends_on.iter().enumerate() {
            if depends_on >= index {
                return Err(format!(
                    "任务「{}」的 depends_on[{dependency_index}] = {depends_on} 非法：\
                     依赖只能引用更早的任务索引（0..{index}）",
                    issue.title
                ));
            }
        }
    }

    Ok(draft)
}

fn is_valid_criterion_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn kind_label(kind: CmdKind) -> &'static str {
    match kind {
        CmdKind::Test => "test",
        CmdKind::Build => "build",
        CmdKind::Lint => "lint",
    }
}

/// 抽取第一个平衡的 `{...}` 片段，忽略字符串内部的花括号与转义字符。
fn extract_json_object(output: &str) -> Result<&str, String> {
    let start = output
        .find('{')
        .ok_or_else(|| "计划输出中未找到 JSON 对象".to_string())?;
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

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&output[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }

    Err("计划输出的 JSON 对象不完整".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> Vec<DiscoveredCommand> {
        vec![
            DiscoveredCommand {
                cmd: "cargo test".to_string(),
                kind: CmdKind::Test,
                source: "Cargo.toml".to_string(),
            },
            DiscoveredCommand {
                cmd: "cargo build".to_string(),
                kind: CmdKind::Build,
                source: "Cargo.toml".to_string(),
            },
        ]
    }

    fn project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "temli".to_string(),
            repo_path: "/tmp/temli".to_string(),
            github_owner: None,
            github_repo: None,
            default_branch: "main".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn issue(title: &str, cmd: &str, depends_on: Vec<usize>) -> serde_json::Value {
        serde_json::json!({
            "title": title,
            "prompt": format!("实现 {title} 并补充测试"),
            "acceptance": [{
                "id": "c-1",
                "cmd": cmd,
                "expect_exit": 0,
                "expect_match": null
            }],
            "depends_on": depends_on
        })
    }

    fn draft_json(issues: Vec<serde_json::Value>) -> String {
        serde_json::json!({ "issues": issues }).to_string()
    }

    #[test]
    fn parses_valid_plan_with_dependency() {
        let output = draft_json(vec![
            issue("解析输入", "cargo test", vec![]),
            issue("接入界面", "cargo build", vec![0]),
        ]);

        let draft = parse_plan(&output, &commands()).expect("合法计划应被接受");

        assert_eq!(draft.issues.len(), 2);
        assert_eq!(draft.issues[0].title, "解析输入");
        assert_eq!(draft.issues[0].acceptance.len(), 1);
        assert_eq!(draft.issues[0].acceptance[0].cmd, "cargo test");
        assert_eq!(draft.issues[1].depends_on, vec![0]);
    }

    #[test]
    fn parses_plan_wrapped_in_markdown_fence_and_prose() {
        let output = format!(
            "好的，计划如下：\n```json\n{}\n```\n以上。",
            draft_json(vec![issue("解析输入", "cargo test", vec![])])
        );

        let draft = parse_plan(&output, &commands()).expect("围栏输出应被接受");

        assert_eq!(draft.issues.len(), 1);
        assert_eq!(draft.issues[0].title, "解析输入");
    }

    #[test]
    fn rejects_empty_duplicate_and_oversized_plans() {
        let err = parse_plan(r#"{"issues":[]}"#, &commands()).unwrap_err();
        assert!(err.contains("至少"), "{err}");

        let duplicate = draft_json(vec![
            issue("重复标题", "cargo test", vec![]),
            issue("重复标题", "cargo build", vec![]),
        ]);
        let err = parse_plan(&duplicate, &commands()).unwrap_err();
        assert!(err.contains("重复"), "{err}");

        let too_many: Vec<_> = (0..=MAX_PLAN_ISSUES)
            .map(|index| issue(&format!("任务 {index}"), "cargo test", vec![]))
            .collect();
        let err = parse_plan(&draft_json(too_many), &commands()).unwrap_err();
        assert!(err.contains("最多"), "{err}");
    }

    #[test]
    fn rejects_unknown_command() {
        let output = draft_json(vec![issue("危险任务", "rm -rf /", vec![])]);

        let err = parse_plan(&output, &commands()).unwrap_err();

        assert!(err.contains("rm -rf /"), "{err}");
        assert!(err.contains("未发现"), "{err}");
    }

    #[test]
    fn rejects_forward_self_and_duplicate_criterion_ids() {
        let forward = draft_json(vec![
            issue("第一个", "cargo test", vec![]),
            issue("第二个", "cargo build", vec![1]),
        ]);
        let err = parse_plan(&forward, &commands()).unwrap_err();
        assert!(err.contains("depends_on"), "{err}");

        let self_reference = draft_json(vec![issue("唯一任务", "cargo test", vec![0])]);
        let err = parse_plan(&self_reference, &commands()).unwrap_err();
        assert!(err.contains("depends_on"), "{err}");

        let duplicate_ids = serde_json::json!({
            "issues": [{
                "title": "任务",
                "prompt": "提示",
                "acceptance": [
                    {"id": "c-1", "cmd": "cargo test", "expect_exit": 0},
                    {"id": "c-1", "cmd": "cargo build", "expect_exit": 0}
                ],
                "depends_on": []
            }]
        })
        .to_string();
        let err = parse_plan(&duplicate_ids, &commands()).unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_plan("没有 JSON，只有说明。", &commands()).unwrap_err();
        assert!(err.contains("未找到"), "{err}");

        let err = parse_plan(r#"{"issues":[{"title":"x"}"#, &commands()).unwrap_err();
        assert!(err.contains("不完整"), "{err}");

        let err = parse_plan(r#"{"issues":[{"title":"x",}]}"#, &commands()).unwrap_err();
        assert!(err.contains("解析失败"), "{err}");
    }

    #[test]
    fn prompt_requires_grounded_commands_and_json_only() {
        let prompt = plan_prompt("做一个搜索面板", &project(), &commands());

        assert!(prompt.contains("做一个搜索面板"), "{prompt}");
        assert!(prompt.contains("temli"), "{prompt}");
        assert!(prompt.contains("cargo test"), "{prompt}");
        assert!(prompt.contains("cargo build"), "{prompt}");
        assert!(prompt.contains("\"depends_on\""), "{prompt}");
        assert!(prompt.contains("ONLY one JSON object"), "{prompt}");
        assert!(prompt.contains("1 to 10"), "{prompt}");
    }
}
