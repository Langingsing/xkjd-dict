use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::{fs, path::PathBuf};
use xkjd_dict::{
    allocate::{self, AllocationDecision, AllocationRequest, PromotionRequest},
    audit::{self, Severity},
    document::Document,
    edit::{EditPlan, Operation, dictionary_path},
    external::{self, ExternalData},
    reverse::{Alternatives, ReverseIndex},
    rules::{self, Encoder, Profile},
    workspace::Workspace,
};

#[derive(Parser)]
#[command(
    version,
    about = "本地键道词库维护与规则检查；使用 --root 指定 Rime 词库目录"
)]
struct Cli {
    #[arg(long, global = true, default_value_os_t = default_root())]
    root: PathBuf,
    #[arg(long, global = true, default_value = "xkjd6")]
    schema: String,
    /// 包含未启用词库；默认仅检查/查询当前 schema 导入的词库
    #[arg(long, global = true)]
    all: bool,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
fn default_root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("fcitx5/rime")
}

#[derive(Clone, Copy, ValueEnum)]
enum RuleArg {
    Standard,
    Initials,
    Css,
    CssSingle,
}
impl From<RuleArg> for Profile {
    fn from(v: RuleArg) -> Self {
        match v {
            RuleArg::Standard => Self::Standard,
            RuleArg::Initials => Self::Initials,
            RuleArg::Css => Self::Css,
            RuleArg::CssSingle => Self::CssSingle,
        }
    }
}
#[derive(Args)]
struct EncodeArgs {
    text: String,
    #[arg(long, value_enum, default_value = "standard")]
    profile: RuleArg,
    /// 单字完整编码覆盖，可重复：--char-code 重=wyuvia
    #[arg(long = "char-code")]
    character_codes: Vec<String>,
}
#[derive(Args)]
struct AllocateArgs {
    /// 前者保留现有简码
    keep: String,
    /// 后者延长到最短空闲合法编码
    extend: String,
    /// 双方现有共享编码；存在多个共享码时必填
    #[arg(long)]
    code: Option<String>,
    /// 后者所在词库；存在多条匹配记录时用于选择
    #[arg(long)]
    file: Option<PathBuf>,
    /// 后者原始行号；与 --file 一起唯一选择记录
    #[arg(long, requires = "file")]
    line: Option<usize>,
    /// 显式选择规则生成的完整编码
    #[arg(long)]
    full_code: Option<String>,
    /// 单字完整编码覆盖，可重复：--char-code 重=wyuvia
    #[arg(long = "char-code")]
    character_codes: Vec<String>,
    /// 显式写入；缺省只预览分配结果和 diff
    #[arg(long)]
    apply: bool,
}
#[derive(Args)]
struct PromoteArgs {
    /// 要取得短码的已有词组
    text: String,
    /// 目标短码；程序查询当前占词并将其延长
    code: String,
    /// 目标词有多个现有码时显式选择
    #[arg(long)]
    from_code: Option<String>,
    /// 目标词组的完整编码（存在歧义时指定）
    #[arg(long)]
    full_code: Option<String>,
    /// 原占词的完整编码（存在歧义时指定）
    #[arg(long)]
    owner_full_code: Option<String>,
    #[arg(long = "char-code")]
    character_codes: Vec<String>,
    #[arg(long)]
    apply: bool,
}
#[derive(Args)]
struct ReverseArgs {
    /// 每类排序最多返回多少个方案（1..=20）
    #[arg(long, default_value = "5", value_parser = clap::value_parser!(u8).range(1..=20))]
    limit: u8,
    /// 用户已实际验证的空格首选：--confirmed-first 会=hb；可重复
    #[arg(long = "confirmed-first")]
    confirmed_first: Vec<String>,
}
#[derive(Args)]
struct Target {
    /// 词库文件名；必须位于 root 的直接子目录层级
    #[arg(long)]
    file: PathBuf,
    /// 显式写入；缺省只预览 unified diff
    #[arg(long)]
    apply: bool,
}

#[derive(Serialize)]
struct EditResult {
    file: PathBuf,
    applied: bool,
    diff: String,
}

#[derive(Subcommand)]
enum Command {
    /// 按前缀和可选词长列出码族；不推测词频
    Group {
        prefix: String,
        #[arg(long)]
        length: Option<std::num::NonZeroUsize>,
    },
    /// 取得已有短码，原占词自动延长；同文件一次保序写入，默认只预览
    Promote(PromoteArgs),
    /// 前者保留共享码，后者延长至最短空闲合法码；默认只预览
    Allocate(AllocateArgs),
    /// 反查已有最短编码及分段输入方案，显式计入每段空格
    Reverse {
        text: String,
        #[command(flatten)]
        options: ReverseArgs,
    },
    /// 分析冲突码位的词组是否适合让出整词码；只报告
    Reclaim {
        code: String,
        #[command(flatten)]
        options: ReverseArgs,
        /// 分段方案相比整词编码加一次空格，最多多按多少键（不含未知选词键）
        #[arg(long, default_value = "2")]
        max_extra_keys: u32,
    },
    Query {
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        prefix: Option<String>,
    },
    Encode(EncodeArgs),
    Suggest(EncodeArgs),
    /// 用已下载且校验 SHA256 的外部拼音、笔顺、IDS 对照单字编码
    VerifySingle {
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        report_dir: Option<PathBuf>,
    },
    /// 检查每个词组汉字是否有单字条目，区分全部缺失和仅未启用库存在
    VerifyCharacters {
        #[arg(long)]
        report_dir: Option<PathBuf>,
    },
    Check {
        /// 同时写入 audit.json 和 audit.md；内容含本地私有词条，请勿直接公开
        #[arg(long)]
        report_dir: Option<PathBuf>,
        #[arg(long)]
        fail_on_error: bool,
    },
    Add {
        #[command(flatten)]
        target: Target,
        text: String,
        code: String,
        /// 分类块/无序词库：插入前的原始词条行号；总行数 + 1 表示末尾
        #[arg(long)]
        before_line: Option<usize>,
    },
    Update {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        line: usize,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        code: Option<String>,
    },
    Remove {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        line: usize,
    },
    /// 同一文件批量编辑，行号均指编辑前文件；输入 JSON 操作数组
    Batch {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        operations: PathBuf,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("错误：{e:#}");
        std::process::exit(1);
    }
}
fn output<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
fn print_alternatives(label: &str, alternatives: &Alternatives) {
    for (ranking, plans) in [
        ("选择负担优先", &alternatives.easier),
        ("编码+确认键优先", &alternatives.fewest_keys),
        ("全部分段有首选证据", &alternatives.confirmed_first),
    ] {
        println!("{label} / {ranking}：");
        if plans.is_empty() {
            println!("  无完整覆盖方案");
        }
        for plan in plans {
            println!(
                "  模板 {:?}：编码 {} + 确认 {} = {} 键；分段边界 {}；候选负担段数 {}（排名未知 {}；已知需选择至少 {} 次）",
                plan.input,
                plan.burden.code_keys,
                plan.burden.confirmation_keys,
                plan.burden.typed_keys,
                plan.burden.boundaries,
                plan.burden.unresolved_selections,
                plan.burden.unknown_rank_segments,
                plan.burden.selection_actions_lower_bound
            );
            for segment in &plan.segments {
                println!(
                    "    {}={}：{:?}；精确同码词 {:?}",
                    segment.text,
                    segment.code,
                    segment.certainty,
                    segment
                        .candidates
                        .iter()
                        .map(|c| &c.text)
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}
fn edit(
    root: &std::path::Path,
    target: &Target,
    operations: &[Operation],
    json: bool,
) -> Result<()> {
    let file = dictionary_path(root, &target.file)?;
    let doc = Document::read(&file)?;
    let plan = EditPlan::build(&doc, operations)
        .with_context(|| format!("准备编辑 {}", file.display()))?;
    finish_edit(plan, target.apply, json)
}
fn finish_edit(plan: EditPlan, apply: bool, json: bool) -> Result<()> {
    let file = plan.path.clone();
    let diff = plan.diff();
    if apply {
        plan.apply()
            .with_context(|| format!("写入 {}", file.display()))?;
    }
    if json {
        output(&EditResult {
            file,
            applied: apply,
            diff,
        })?;
    } else {
        print!("{diff}");
        println!(
            "{}",
            if apply {
                "已写入；未执行部署或 Git 操作。"
            } else {
                "仅预览；添加 --apply 才会写入。"
            }
        );
    }
    Ok(())
}
fn finish_allocation(decision: AllocationDecision, apply: bool, json: bool) -> Result<()> {
    let report = match decision {
        AllocationDecision::Ready(mut plan) => {
            if apply {
                plan.apply()?;
            }
            plan.report
        }
        AllocationDecision::Blocked(report) => report,
    };
    if json {
        output(&report)?;
    } else {
        if let Some(issue) = &report.issue {
            println!("无法分配：{}", issue.message);
            println!("共享码候选：{:?}", report.shared_candidates);
            for entry in &report.moving_candidates {
                println!(
                    "后者记录：{} {}  {}:{}",
                    entry.text,
                    entry.code,
                    entry.file.display(),
                    entry.line
                );
            }
            if let Some(suggestion) = &report.suggestion {
                println!(
                    "全码候选：{:?}；歧义字：{:?}；缺失字：{:?}",
                    suggestion.encoding.candidates,
                    suggestion.encoding.ambiguous,
                    suggestion.encoding.missing
                );
            }
            if let Some(encoding) = &report.promotion_encoding {
                println!(
                    "目标全码候选：{:?}；歧义字：{:?}；缺失字：{:?}",
                    encoding.candidates, encoding.ambiguous, encoding.missing
                );
            }
        } else {
            if let Some(from) = &report.promoted_from {
                println!(
                    "{} {} → {}",
                    report.keep,
                    from,
                    report.shared_code.as_deref().unwrap_or_default()
                );
                println!(
                    "{} {} → {}",
                    report.extend,
                    report.shared_code.as_deref().unwrap_or_default(),
                    report.assigned_code.as_deref().unwrap_or_default()
                );
            } else {
                println!(
                    "{} 保留 {}；{} 改为 {}（全码 {}）",
                    report.keep,
                    report.shared_code.as_deref().unwrap_or_default(),
                    report.extend,
                    report.assigned_code.as_deref().unwrap_or_default(),
                    report.full_code.as_deref().unwrap_or_default()
                );
            }
            print!("{}", report.diff);
            println!(
                "{}",
                if apply {
                    "已写入并验证；未执行部署或 Git 操作。"
                } else {
                    "仅预览；添加 --apply 才会写入。"
                }
            );
        }
    }
    if report.issue.is_some() {
        bail!("分配未执行；请根据报告明确参数");
    }
    Ok(())
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    // File maintenance does not require a working schema or all other files to parse.
    match &cli.command {
        Command::Add {
            target,
            text,
            code,
            before_line,
        } => {
            let file = dictionary_path(&cli.root, &target.file)?;
            let doc = Document::read(&file)?;
            let plan = EditPlan::insert(&doc, text, code, *before_line)
                .with_context(|| format!("准备插入 {}", file.display()))?;
            return finish_edit(plan, target.apply, cli.json);
        }
        Command::Update {
            target,
            line,
            text,
            code,
        } => {
            return edit(
                &cli.root,
                target,
                &[Operation::Update {
                    line: *line,
                    text: text.clone(),
                    code: code.clone(),
                }],
                cli.json,
            );
        }
        Command::Remove { target, line } => {
            return edit(
                &cli.root,
                target,
                &[Operation::Remove { line: *line }],
                cli.json,
            );
        }
        Command::Batch { target, operations } => {
            let raw = fs::read_to_string(operations)
                .with_context(|| format!("读取批量操作 {}", operations.display()))?;
            let operations: Vec<Operation> =
                serde_json::from_str(&raw).context("解析 JSON 操作数组")?;
            return edit(&cli.root, target, &operations, cli.json);
        }
        _ => {}
    }
    let ws = Workspace::load(&cli.root, &cli.schema)?;
    let concise_suggest = matches!(&cli.command, Command::Suggest(_));
    match cli.command {
        Command::Allocate(args) => {
            let request = AllocationRequest {
                keep: args.keep,
                extend: args.extend,
                code: args.code,
                file: args.file,
                line: args.line,
                full_code: args.full_code,
                character_codes: args.character_codes,
                all: cli.all,
            };
            finish_allocation(allocate::plan(&ws, &request)?, args.apply, cli.json)?;
        }
        Command::Promote(args) => {
            let request = PromotionRequest {
                text: args.text,
                code: args.code,
                from_code: args.from_code,
                full_code: args.full_code,
                owner_full_code: args.owner_full_code,
                character_codes: args.character_codes,
                all: cli.all,
            };
            finish_allocation(allocate::promote(&ws, &request)?, args.apply, cli.json)?;
        }
        Command::Group { prefix, length } => {
            let length = length.map(std::num::NonZeroUsize::get);
            if cli.json {
                output(&ws.group(&prefix, length, cli.all)?)?;
            } else {
                print!("{}", ws.group_tree(&prefix, length, cli.all)?);
            }
        }
        Command::Reverse { text, options } => {
            let index = ReverseIndex::new(&ws, cli.all, &options.confirmed_first)?;
            let result = index.lookup(&text, usize::from(options.limit))?;
            if cli.json {
                output(&result)?;
            } else {
                println!(
                    "{}：已有最短编码 {:?}",
                    result.text, result.shortest_existing
                );
                for entry in &result.existing {
                    println!(
                        "  {}：{:?}；精确同码词 {:?}",
                        entry.code,
                        entry.certainty,
                        entry.candidates.iter().map(|c| &c.text).collect::<Vec<_>>()
                    );
                }
                print_alternatives("允许整词", &result.alternatives);
                print_alternatives("仅分段（不依赖整词码）", &result.segmented);
                for issue in &result.import_issues {
                    println!("导入问题 {}：{}", issue.file.display(), issue.message);
                }
                println!("范围 {:?}；{}", result.scope, result.note);
            }
        }
        Command::Reclaim {
            code,
            options,
            max_extra_keys,
        } => {
            let index = ReverseIndex::new(&ws, cli.all, &options.confirmed_first)?;
            let result = index.reclaim(&code, usize::from(options.limit), max_extra_keys)?;
            if cli.json {
                output(&result)?;
            } else {
                println!(
                    "码位 {}：{} 个词组；允许额外键数 {}",
                    result.code,
                    result.candidates.len(),
                    result.max_extra_keys
                );
                for candidate in &result.candidates {
                    println!(
                        "{}：{:?}；分段额外键数 {:?}",
                        candidate.text, candidate.assessment, candidate.extra_keys
                    );
                    if let Some(plan) = &candidate.recommended {
                        println!("  对应评估方案：{:?}", plan.input);
                    }
                    print_alternatives("仅分段", &candidate.segmented);
                }
                for issue in &result.import_issues {
                    println!("导入问题 {}：{}", issue.file.display(), issue.message);
                }
                println!("范围 {:?}；{}", result.scope, result.note);
            }
        }
        Command::VerifyCharacters { report_dir } => {
            let report = xkjd_dict::coverage::check(&ws, cli.all);
            if let Some(dir) = report_dir {
                fs::create_dir_all(&dir)?;
                fs::write(
                    dir.join("phrase-characters.json"),
                    serde_json::to_vec_pretty(&report)?,
                )?;
                fs::write(
                    dir.join("phrase-characters.md"),
                    format!("# 词组用字集合检查\n{}", report.markdown()),
                )?;
            }
            if cli.json {
                output(&report)?;
            } else {
                println!(
                    "检查 {} 条词组；全部单字集合 {} 字，启用单字集合 {} 字；全部缺字词组 {} 条，仅未启用库有字的词组 {} 条",
                    report.phrases_checked,
                    report.all_single_characters,
                    report.active_single_characters,
                    report.absent_from_all.len(),
                    report.only_in_inactive.len()
                );
            }
        }
        Command::VerifySingle {
            data_dir,
            report_dir,
        } => {
            let data = ExternalData::load(&data_dir)?;
            let report = external::verify(&ws, &data, cli.all);
            if let Some(dir) = report_dir {
                fs::create_dir_all(&dir)?;
                fs::write(
                    dir.join("single-verification.json"),
                    serde_json::to_vec_pretty(&report)?,
                )?;
                fs::write(dir.join("single-verification.md"), report.markdown())?;
            }
            if cli.json {
                output(&report)?;
            } else {
                println!(
                    "外部交叉验证：{} 个汉字，{} 条本地单字记录",
                    report.characters.len(),
                    report.local_entry_count
                );
                for (status, n) in &report.sound_counts {
                    println!("音码 {status:?}: {n}");
                }
                for (status, n) in &report.shape_counts {
                    println!("形码 {status:?}: {n}");
                }
            }
        }
        Command::Query { text, code, prefix } => {
            if text.is_none() && code.is_none() && prefix.is_none() {
                bail!("query 至少提供 --text、--code 或 --prefix");
            }
            let entries = ws.query(text.as_deref(), code.as_deref(), prefix.as_deref(), cli.all);
            if cli.json {
                output(&entries)?;
            } else {
                for e in &entries {
                    println!("{}\t{}\t{}:{}", e.text, e.code, e.file.display(), e.line);
                }
                println!(
                    "{} 条记录；顺序为来源文件/行号，不表示 Rime 排名。",
                    entries.len()
                );
            }
        }
        Command::Encode(args) | Command::Suggest(args) => {
            let encoder = Encoder::from_workspace(&ws).with_overrides(&args.character_codes)?;
            let result = rules::suggest(&ws, &encoder, &args.text, args.profile.into(), cli.all);
            if cli.json {
                output(&result)?;
            } else if concise_suggest {
                print!("{}", result.concise_text());
            } else {
                println!(
                    "{}：候选 {}；已有记录 {}",
                    args.text,
                    result.encoding.candidates.len(),
                    result.existing.len()
                );
                if !result.encoding.missing.is_empty() {
                    println!("参考缺失：{:?}", result.encoding.missing);
                }
                for issue in &result.encoding.issues {
                    println!("{issue}");
                }
                if !result.encoding.ambiguous.is_empty() {
                    println!(
                        "多音/飞键歧义：{:?}；可用 --char-code 指定",
                        result.encoding.ambiguous
                    );
                }
                if result.encoding.truncated {
                    println!("候选超过上限，结果不完整。");
                }
                for choice in result.choices {
                    println!(
                        "全码 {}；最短空闲码 {}",
                        choice.full_code,
                        choice.shortest_free.as_deref().unwrap_or("无")
                    );
                    for s in choice.slots {
                        println!(
                            "  {}：同码 {} 条，后继 {} 条，已占用前缀 {:?}",
                            s.code,
                            s.same_code.len(),
                            s.descendant_entries,
                            s.ancestor_codes
                        );
                        for e in s.same_code {
                            println!("    {}  {}:{}", e.text, e.file.display(), e.line);
                        }
                    }
                }
                println!("{}", result.ordering_note);
            }
        }
        Command::Check {
            report_dir,
            fail_on_error,
        } => {
            let encoder = Encoder::from_workspace(&ws);
            let report = audit::check(&ws, &encoder, cli.all);
            if let Some(dir) = report_dir {
                fs::create_dir_all(&dir)?;
                fs::write(dir.join("audit.json"), serde_json::to_vec_pretty(&report)?)?;
                fs::write(dir.join("audit.md"), report.markdown())?;
            }
            if cli.json {
                output(&report)?;
            } else {
                println!(
                    "扫描 {} 文件、{} 记录；确定问题 {} 项，待核对 {} 项，信息 {} 项；同码多词节点 {}。",
                    report.files.len(),
                    report.records,
                    report.count(Severity::Error),
                    report.count(Severity::Review),
                    report.count(Severity::Info),
                    report.shared_nodes.len()
                );
                for (p, c) in &report.coverage {
                    println!(
                        "{p:?}: {} 记录，匹配 {}，问题 {}，待核对 {}，不适用 {}",
                        c.records, c.matched, c.errors, c.review, c.not_applicable
                    );
                }
            }
            if !cli.json {
                println!(
                    "词组用字：{} 条全部单字库缺字，{} 条用字仅存在于未启用单字库",
                    report.character_coverage.absent_from_all.len(),
                    report.character_coverage.only_in_inactive.len()
                );
            }
            if fail_on_error && report.count(Severity::Error) > 0 {
                bail!("检查发现确定问题；报告已生成");
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
