# xkjd-dict

本地键道词库维护工具，Rust CLI 与 library。词库文件是唯一数据源，索引每次从文件重新建立。提供无损编辑、编码建议、前缀树查询、规则检查及编码反查；词库由使用者自行提供，本仓库不分发完整词库。

Rust CLI 是主要入口；Python 跟打和桌面采集属于 Linux 实验功能，有额外的输入法、终端和桌面依赖。

## 构建与使用

```bash
cd xkjd-dict
cargo build --release --locked
./target/release/xkjd-dict --help
cargo install --path . --locked
```

需要支持 Rust 2024 edition 的 Rust 工具链，建议使用当前 stable。`cargo install` 将命令安装到 Cargo 的 bin 目录，须将其加入 PATH。默认目录为 `$XDG_DATA_HOME/fcitx5/rime`，未设置时为 `$HOME/.local/share/fcitx5/rime`，默认 schema 为 `xkjd6`。其他 Rime 安装位置可用 `--root`、`--schema` 覆盖。

```bash
xkjd-dict query --text 乱画
xkjd-dict query --code lthqu
xkjd-dict query --prefix lthq
xkjd-dict encode 词库管理
xkjd-dict suggest 重庆 --char-code 重=wyuvia
xkjd-dict encode 你好 --profile css
xkjd-dict --json encode 词库管理
xkjd-dict verify-characters --all --report-dir /path/to/report
xkjd-dict check --all --report-dir ./outputs
```

`query` 默认按字面完全匹配，`--prefix` 按编码前缀匹配；组合条件取交集。输出按文件、行号排序，不是 Rime 候选顺序。默认查询和码位建议仅使用 schema 导入的词库；`--all` 包含未启用词库。`suggest` 默认只显示最短可用码，以及通往该码的合法短码上已有的词，便于决定插入新码还是让出更短码；已存在的词显示现有码，存在更短空闲码时才额外建议。缺字、歧义、候选不完整或无空闲码仍明确提示。`encode` 保留完整文本说明，`suggest --json` 与 `encode --json` 保留完整证据、来源和前缀影响字段。

## 词组同码分配

查询某个前缀下的词组并调整短码归属：

```bash
xkjd-dict group lhq --length 3
xkjd-dict promote 零花钱 lhq
xkjd-dict promote 零花钱 lhq --apply
```

`group` 默认以树形展示前缀和逐键分支，同码词合并显示，空的中间节点也保留；`--length` 可选。JSON 仍输出完整词条列表及来源，不合并记录。它不推测词频或候选顺序。

`promote TEXT CODE` 让一个已有词组取得更短的合法编码，自动查询此码的唯一原占词，将原占词延长到最短空闲合法编码，一次预览或写入两条保序修改。目标原码释放后也可供原占词使用。双方须在同一文件；空码、多个占词、重复来源、目标已拥有该码或需要跨文件修改时拒绝，不隐式处理其他词条。目标存在多个现有码时用 `--from-code` 选择；双方取码有歧义时分别用 `--full-code`（目标）、`--owner-full-code`（原占词）指定合法全码，或用 `--char-code` 明确单字参考。默认展示 diff，只有 `--apply` 写入；库入口为 `allocate::promote`，复用下述占用、排序、并发与记录保真检查。上述命令是一次调换的预览和写入示例，成功后重复执行会拒绝。

`allocate` 按参数顺序让前者保留现有简码，后者延长到严格长于原码的最短空闲合法编码。一次命令完成查询、一次取码与建议计算、占用检查、保序编辑和验证，默认只显示结果与 diff：

```bash
xkjd-dict allocate '保留的词组' '延长的词组'
xkjd-dict --json allocate '保留的词组' '延长的词组'
# 根据预览明确参数后，显式写入：
xkjd-dict allocate '保留的词组' '延长的词组' --apply
```

双方有多个共享码时用 `--code` 指定原码；后者存在多条记录时用 `--file` 和 `--line` 选择原始记录。读音、飞键或取码有歧义时停止，JSON 保留全码候选及单字证据，用 `--full-code` 选择合法全码，或重复传入 `--char-code 字=完整编码` 明确参考；不因某个歧义候选恰好空闲就自动采用它。未知单字、候选截断、原码还有第三方占用、现有码不是合法前缀或所有延长码被占用时明确返回；阻塞结果的 JSON 包含 `status: blocked` 和 `issue.reason`，进程返回非零。

目前只调整普通键道词组库中的一条已有记录，不新增、合并或删除词条。占用范围默认是当前 schema 导入词库，`--all` 扩展到未启用库。注释和空行是分类边界，只在原有序分类块内稳定调整词条位置；若必须跨块或原块无序则拒绝。保留原 YAML、注释、同码相对顺序、额外字段、换行和尾行有无换行，并验证除目标编码外记录字段集合不变。前者及其他文件不写入。

预览和写入前复核所加载的词库文件集合、内容和原始 schema；包括取码参考和其他占用库，发生变化便拒绝旧计划。沿用无损编辑器的原子替换和乐观并发检查，不提供跨文件事务锁。写入后再核对目标文件；不部署输入法，不自动执行 Git 提交或推送。库入口为 `allocate::plan`，返回可预览的 `AllocationDecision`，仅 `AllocationPlan::apply` 写入。

## 编码反查、分段输入与让码候选

```bash
xkjd-dict reverse 会看
xkjd-dict --json reverse 会看 --limit 3
xkjd-dict reverse 会看 --confirmed-first 会=hb --confirmed-first 看=kf
xkjd-dict reclaim hbkf --max-extra-keys 2
```

`reverse` 从已选词库反查现有编码，按码长排列并返回所有并列最短码；它不生成尚未收录的码。分段使用整个文本的动态规划，每类最多返回 `--limit` 个方案（默认 5，上限 20），不是贪心选最长词。支持 1～128 个 Unicode 字符，没有覆盖方案时明确返回空列表，不猜测缺字或插入原文。只使用小写字母编码，拼音和日语字典不提供分段边；任何已选同码记录仍计入候选竞争。默认范围是原始 schema 的导入词库，`--all` 为包含未启用库的假设分析。

- 选择负担优先：依次最小化需要核对或选择候选的段数、编码加确认键数、分段边界数。
- 编码加确认键优先：交换前两项，方便比较少按键但可能需要选词的方案。
- 全部分段有首选证据：仅使用用户已确认首选的编码，再按编码加确认键排序。没有足够证据时为空，不把静态唯一性混作实测。

每个分段按一次空格确认，包括最后一段。`burden` 分开给出编码键数、确认键数、两者合计、分段边界、候选负担段数、排名未知段数，以及已知非首选所需选择动作的下界。未知排名不虚构为一次必需选词；数字选词可能替代空格、候选页数和具体选择键序列不在本模型内，实际总键数不能由静态词库精确推算。输出中的 `[核对候选:词语]` / `[选择:词语]` 是操作占位，不能直接照打。

同一编码的同一词语即使有多条来源也只算一个候选，JSON 保留所有文件和行号。同码不同词不按文件顺序猜测排名：`unique_exact` 表示唯一精确字典候选，`unverified_rank` 表示多个候选且首选未知，`confirmed_first` 表示用户明确提供了已实测的空格首选；若用户确认了另一个词为首选，该段为 `needs_selection`。`--confirmed-first 词语=编码` 必须对应已选词库记录，同一码的矛盾首选会被拒绝。这些证据仅作用于当前调用，不自动保存，也不代表程序进行了 Rime 实测。

例如已知 `hb 空格` 上屏“会”、`kf 空格` 上屏“看”，传入上述首选证据后，“会看”的分段方案为 `hb kf `：编码 4 键、确认 2 键、1 个分段边界、无待核对选择。未提供首选证据时，本地 `hb` 的“会/悔”重码会明确保留为排名未知。分段方案独立于“会看”整词码是否存在。

`reclaim CODE` 只分析**当前占用该码位的词组**，不生成全库清理名单，不修改任何词条。每个词组的替代方案强制至少两段，排除全部整词编码，避免把待让出的短码算回替代方案。相对整词编码加一次空格，默认允许额外 2 个编码/确认键，可用 `--max-extra-keys` 调整。返回：

|assessment|含义|
|---|---|
|confirmed_candidate|全部分段具有用户首选证据，且额外键数在阈值内，可人工考虑让码|
|static_candidate|无已知候选负担且键数在阈值内，但部分分段仅有词库唯一性证据，仍需实测|
|needs_selection|分段中存在排名未知或已知非首选，暂不视作省心候选|
|extra_keys|分段的额外编码/确认键数超过阈值|
|no_segmentation|没有不依赖整词码的完整分段方案|
|inactive_scope / incomplete_imports|分析包含未启用库，或当前导入不完整，不能作为当前输入法的让码依据|

若存在额外键数在阈值内的全段首选已确认方案，让码分析优先采用它；否则采用选择负担优先方案。`recommended` 和 `extra_keys` 对应同一评估方案，其余方案仍保留以供比较；因此更短的静态方案或较小的 `--limit` 不会遮住可用的已确认方案。

本机 `hbkf` 现已属于“回看”，因此 `reclaim hbkf` 分析的是“回看”；要查看已让出该码的“会看”，用 `reverse 会看`。让码建议不会调整现有候选次序、替换整词或提交 Git。动态用户词典、补全、custom patch、过滤器、顶功和配置切换均可能改变实际上屏，静态候选唯一不等于运行时保证。长期可在同一库接口之上接入实测候选和顶功证据，当前不以码长代替上屏确定性，也不把分段成本压成一个未经校准的手感分数。

## 修改文件

用户词库按用途拆为 `compose/user/subject/media/life.xkjd6.dict.yaml`，对应组字成词、个人偏好、学科、影音媒体、生活；普通补充词组并入 `xkjd6.cizu.dict.yaml`，人名库为 `name.xkjd6.dict.yaml`。拆出文件全文按编码稳定升序，人名库保留分类注释并在块内稳定排序。原 `xkjd6.user`、`xkjd6.names` 文件和导入项已移除。

先用 query 获取行号。所有修改默认只预览 diff，显式添加 `--apply` 才写入。每条命令只修改一个已存在的词库文件。

```bash
xkjd-dict add --file xkjd6.cizu.dict.yaml 好运来 hyla
xkjd-dict add --file name.xkjd6.dict.yaml 姓名示例 xmsl --before-line 100
xkjd-dict update --file user.xkjd6.dict.yaml --line 10 --code lthqu
xkjd-dict remove --file user.xkjd6.dict.yaml --line 10
xkjd-dict batch --file user.xkjd6.dict.yaml --operations operations.json
```

批量输入为 JSON 数组，所有行号均指修改前的文件；同一行不能重复修改：

```json
[
  {"op": "update", "line": 10, "code": "lthqu"},
  {"op": "remove", "line": 11},
  {"op": "add", "text": "词库管理", "code": "ckgloo"}
]
```

注释、未改动行、额外列和换行保留。`add` 和批量 `add` 在全文编码有序、没有内部分类分块的词库中按编码插入，同码时排在已有同码词之后；原有词条不重排，尾注仍留在尾部。包含分类块、编码无序或格式异常时拒绝猜测位置，使用 `add --before-line N` 明确插在原始词条 N 之前，`总行数 + 1` 表示文件末尾。显式编码允许人工特殊码，程序不自动采用建议、不自动解决冲突。写入前比对原始内容，先完整写临时文件，再原子替换，保留文件权限并同步文件与目录。它是乐观并发检查，不是对所有外部编辑器加锁；生成计划后若文件已变化会拒绝写入。符号链接和目录外目标被拒绝。操作不执行 Git、部署或词频调整。

## 规则及覆盖边界

普通规则来自[键道词组规则](https://keytao-docs.vercel.app/guide/start-xkjd/phrases.html)、[飞键](https://keytao-docs.vercel.app/guide/advance-in-xkjd/alt-code.html)及[词库编码规则](https://keytao-docs.vercel.app/guide/master-xkjd/dict-types.html)。实际数据以本地旧版码表为准，不把新版词库规则强套到特殊旧库。

- 单字参考取本地 `xkjd6dz.dict.yaml` 与生僻字库中每读音的最长编码。保留多音、飞键及同长度变体，候选保留文件和行号。缺失读音不猜测；`--char-code 字=完整音形码` 可以显式选择。未收录的飞键不会凭空补齐。
- 二字：双拼加两字形码首码；三字：三个声母加三个形码首码；四字及以上：前三字和末字声母，加前两字形码首码。简码为规则允许长度的前缀，二字普通简码允许从三码开始；两码声声另归特殊库。
- 声笔笔和声笔笔单字按各自的取码及码长验证；声声按各字声母验证。
- 纯形码、声音形笔、符号、旧英文助记、诗词 `i` 引导与自定义库只检查有依据的结构；具体映射缺少独立依据时逐条列为待核对。诗词重码后缀可超过普通六位，不设置未经证实的上限。
- 拼音与日语只检查文件和字段格式，编码规则覆盖记为“不适用”。未知文件类型归自定义待核对，不默认为通过。

`check --all` 扫描词库目录直接包含的全部 `.dict.yaml`，不读取 `build/`、`sync/` 的生成物。Rime 常规字典引用按同目录文件名解析，跨目录引用明确报告不支持。schema 使用原始 `translator.dictionary` 和递归 `import_tables`，不应用 custom patch、Lua 或运行时用户词典。

结果分为：

|级别|含义|
|---|---|
|error|格式、结构错误或与本地最长参考码矛盾；参考矛盾不等于独立确认了笔顺/读音|
|review|可能错码、参考读音缺漏、特殊规则映射尚未验证，不能自动修复|
|info|完全重复词条等信息，跨方案重复可能有意|

报告分别统计文件、记录和诊断项；同一记录可能有多项诊断。相同编码映射不同词不是规则错误，另列 `shared_nodes`，附全部来源、启用条数及后继条目数。包含不同方案时不能直接当作当前输入法重码数。`occupied_prefix_nodes` 统计有后继记录的终结码位。

参考表自身的系统性错码无法通过自比对发现；匹配某个读音不代表确认了词义语境。最短空闲码不等于实际最少击键，未模拟 Rime 候选排名、补全与顶功。报告中的 name 不一致是元数据一致性问题，不宣称一定导致编译失败。

`--report-dir` 写出完整 `audit.json` 和 `audit.md`，包含本地私有词条，不宜直接公开。默认检测有问题也正常退出，CI 可加 `--fail-on-error`。查询、编码、检查均不修改词库。

## 外部拼音、笔顺交叉验证与词组用字集合

```bash
python3 scripts/fetch_external.py
xkjd-dict verify-single --all \
  --data-dir ./.cache/external \
  --report-dir /path/to/report
xkjd-dict verify-characters --all --report-dir /path/to/report
```

下载器只下载数据，不接触词库。项目修订和 Unicode 版本固定；CC-CEDICT 是保留本地副本及哈希的滚动快照，只有显式 `--refresh` 才重新下载已有合格缓存。校验器核对每个文件的 SHA256，再解析原始格式。缺失、损坏或格式异常会中止，不用空数据冒充验证成功。

`verify-single` 对全部非拼音/日语字典中的汉字单字记录按（字、编码、规则类型）归并，保留每条原始位置。音码从外部拼音按公开声韵规则独立计算，不调用本地单字编码反查器；多音、ü/省写、零声母、ch/zh 避重及飞键按同一规则处理。`supported` 仅表示至少一份原字外部读音支持，不是“所有来源一致”或“读音已穷尽”。

来源为 Unicode Unihan 17、CC-CEDICT、cnchar 与 Make Me a Hanzi。Unihan 多属性不重复计票；Make Me a Hanzi 字典派生自 Unihan/CJKlib，cnchar 网络数据的独立性未确定。所有数据的 URL、修订、授权、内容哈希及词条定位保留在报告。CC-CEDICT 使用 CC-BY-SA-4.0；cnchar 为 MIT；Make Me a Hanzi dictionary 为 LGPL-3.0-or-later，未下载另行授权的 graphics 数据。

异体关系仅作额外证据：原字无支持、但 Unihan 关联的简繁/异体字读音支持时，记 `variant_reading_review`，不合并字符、不自动继承读音。缺失外部读音、无法映射的读音和外部冲突分别记录；外部冲突仍不等于排除全部姓氏、古音和方言。

形码部分是有限验证：公开字根本身检查完整规则码的前缀；其他字只比较首形码与外部笔顺及可能前置字根的保守集合。缺失的偏旁笔顺可由 Make Me a Hanzi 的 IDS/matches 和 cnchar 笔顺组合推导，推导来源记录在 `root_signatures`。推导要求笔画数一致和部件笔画连续；它不决定键道二分、交错及取大优先。`first_shape_compatible` 不代表全形码通过，地区笔顺或源数据差异保留待核对。cnchar 的“点2”标记含义不明确，不强行解释。

`verify-characters` 遍历词组的每一个汉字，包含不参与取码的长词中间字，报告缺字及从 1 开始的 Unicode 字符位置；单字集合排除拼音/日语词典，保留全部单字集合与启用单字集合的差异。标点、数字、字母及 emoji 不作为缺失汉字。普通 `check` 的 JSON 和 Markdown 也包含这一结果，CLI 有独立汇总。集合存在性不验证读音或编码正确性。

## Library

```rust,no_run
use std::path::Path;
use xkjd_dict::{audit, rules::{Encoder, Profile}, workspace::Workspace};

let ws = Workspace::load(Path::new("/path/to/rime"), "xkjd6")?;
let encoder = Encoder::from_workspace(&ws);
let candidates = encoder.encode("词库管理", Profile::Standard);
let report = audit::check(&ws, &encoder, true);
# Ok::<(), anyhow::Error>(())
```

`Document` 保留原文和记录来源；`EditPlan::build` 接受一组具名 `Operation`，`diff` 生成差异，`apply` 执行变更检测及替换。错误返回包含操作或路径上下文。没有后台服务和隐藏持久化索引。

## 验证与设计取舍

```bash
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

借鉴 smart-dict 的编码 Trie 和反查思想，重新实现安全索引与编辑层。其旧 Trie 使用可移动 HashMap 中节点的裸父指针及生命周期转换，不适合扩展为可靠维护工具；旧文件删除实现还有丢失尾部的风险。词库最多几位的编码无需为压缩树引入这些复杂度。Rust 分段反查复用现有词库索引，以无环分段图的前 K 个最优方案控制组合数；CLI 跟打另用下述 librime 回放验证实际候选及自动顶屏。

## CLI 跟打

实验支持环境为 Linux、KDE Plasma / Konsole、Fcitx5/Rime 和 systemd 用户会话；未验证其他桌面、macOS 或 Windows。Python 需要 3.11 或更新版本，依赖列在 [requirements.txt](requirements.txt)。推荐通过发行版包管理器安装 Python、Rich、PyYAML、dbus-python、PyGObject，以及 Konsole、Fcitx5/Rime、Qt Fcitx 插件、D-Bus、systemd、rime_deployer、g++、pkg-config、librime 和 nlohmann-json。原生库和插件路径目前按 Linux 常见安装布局处理，具体限制见 [模拟器说明](tools/rime-sim/README.md)。

也可在已具备原生开发依赖的环境中使用 `python3 -m pip install -r requirements.txt`；dbus-python 和 PyGObject 可能需要发行版的开发包，普通 pip 安装不替代系统依赖。使用虚拟环境时，后续启动和测试须使用同一环境。

从仓库根目录启动，或将仓库 `scripts/` 的绝对路径加入 PATH 后使用 `xkjd-type`：

```bash
python3 scripts/xkjd-type --help
python3 scripts/xkjd-type --text '回看又视'
```

```bash
xkjd-type --file /path/to/article.txt
xkjd-type --file /path/to/articles
xkjd-type --file /path/to/article.txt --paragraph 2
xkjd-type --text '回看又视'
xkjd-type --file /path/to/article.txt --plan-only
xkjd-type --report-log /tmp/xkjd-type-log-EXAMPLE/round-0001.json
```

跟打界面和完成结果都显示所选文章的绝对路径。参考不可用时，结果页显示失败的句段和具体原因；编码按编译后 schema 的 `speller/alphabet` 索引，包括 `;d` 等标点码。英文组字也参与真实 Rime 回放验证；小写英文可按 Enter 原样上屏，必要时分段以避开汉字顶屏，不切换英文模式。

`--file` 接受文件或目录。目录会递归查找所有层级的 `.txt`、`.md`、`.markdown` 文件（扩展名不区分大小写），从全部候选中随机选一篇，并显示选中文件路径；没有候选时明确报错。文件默认整篇加载，不自动分段或跳到下一段；只有显式提供 `--paragraph N` 才按空行选取选中文章的第 N 段。读取 UTF-8，统一换行并去掉首尾换行。稳定入口为 `scripts/xkjd-type`，本机链接至 `~/.local/bin/xkjd-type`；实现位于 `tools/trainer/`，原生回放接口复用 `tools/rime-sim/`。

启动后切到新开的 Konsole，等待输入法准备好即可输入，无需等待参考路径。界面由 Rich 的 Layout、Panel、Table、Text 和 Live 渲染，使用本机系统包 `python-rich`，沿用你的 Konsole 字体和配色。原文在正文区内左对齐，已打正确文字置灰，错字位置标红；正文始终保留原文，不显示替换字、下划线或独立错字提示。原文连续排列，使用终端正常行距，整个正文区下方只留一个固定输入区；换行时原文行位不变，也不滑动下一行。超出当前显示范围时整页切换。终端光标锚定在输入区，横向跟随当前原文位置，供原生组字与候选框显示。底部固定显示进度和用时，窗口缩放时重新排版。全文打对立即停止计时和接收正文编辑，自动显示详细统计与答案，无需额外按 Enter 或结束键；结束后的按键不计入本次效率。左右、Home/End、Backspace、Delete、Ctrl+W 用于正文编辑。

完成后用表格对齐实际、最短和少选重方案的总键、码键、确认和选重，另列输入法退格、正文删除键及实际删字数。最短方案先最少按键，同键数再少选重；少选重方案先最少次选/三选，同选重数再少按键。确认次数和上屏分段独立展示，不合成“省心分数”。实际序列分别与两个参考按上屏与编辑记录对照，显示“原文／实际／最短”与“原文／实际／少选重”三行 token。原文的每个片段与其实际及参考按键在同一列；分段不同时合并共同边界，不把一个字词的码键硬分摊给单字。相同置灰，实际不同标红，参考不同标绿，缺位以 `·` 显示；这表示按键差异，不代表该按键一定打错。已删除的错误上屏及尾部按键在“删改”行单列，不重复计入原文片段。对照按原文位置排列，不把修改后的片段顺序冒充原始时间顺序。长序列采用有界分块对齐并标明，不声称最少编辑次数。任一参考不可用时，另一参考仍可独立比较。按实际上屏和正文编辑分别定位两个方案的额外操作；跨越不同上屏分段时合并区间，避免重复归因。顶屏键可能开启下一词，已删除的错误上屏另计；不做语义断词或因果猜测。无法完整映射原文时明确标明并回退为整序列 diff，保留全部按键；采集无效时只列参考和已捕获指标。

结果先显示实测用时、击键数、键/秒、键/分钟、字符/分钟、总键/字符和码键/字符，随后用一张紧凑表显示方案指标和左右手分布，再显示两组原文与编码对照。计时从首次原生主键开始，到全文正确结束冻结，等待参考及查看结果不计时；参考没有实测时间，不推测速度。字符按原文 Unicode 码点计，包含标点和换行。左右手按标准 QWERTY 指法估计，百分比分母为左手加右手键数，空格、无法判定的键单列，独立修饰键不计入主键数。指标不用于推断实际指法。未完成、粘贴或采集异常时不显示速度、效率差异或实际 diff。

跟打时只显示左对齐的原文、进度和用时，错误仅改变原文颜色。结果页不设大外框，不限制为 96 列，使用终端可用宽度；可用宽度至少 120 列时，两组对比并排显示，否则上下排列。颜色图例、指法估计口径和操作提示固定在 TUI 底部，数据区不反复展示说明段落。结果区显示当前位置和是否到达末尾；↑↓ / PgUp/PgDn 滚动，Home/End 到首尾，翻页步长随窗口高度调整，操作提示固定在底部。窄窗口自动换行，原文、结果和异常使用一致的视觉层级。

显式 UI 验证脚本 `tools/trainer/desktop_smoke.py` 的 `--reference-log` 参数可指定已有 `oracle.references.json` 回放参考路径；脚本仍需指定 `--session`、`--evidence`、`--return-window` 并运行在 UI guard 下。此模式是程序回放，不代表本人历史实际按键或速度，记录标为 `automation`。正常跟打入口不注入按键，记录标为 `user`。旧版日志没有本人实际按键，无法补回历史轮次。

F3 重练额外操作最多的片段，只在本次会话内切换目标；↑↓ / PgUp/PgDn 滚动结果。F2 或 Ctrl+D 提前退出跟打或关闭结果。完成轮次保存到本次临时日志目录，不恢复上次任务；未完成退出不保存实际轮次。运行所需的 IPC 和隔离输入法文件正常退出或受控失败后清理。每次启动在 `/tmp/xkjd-type-log-*` 留下诊断日志（目录权限 700），包括源文件路径、逐句参考的原文、状态、耗时、按键路径及最终回放结果；`oracle.jsonl` 逐句追加，`oracle.references.json` 保存最近一次完整参考，`reference.log` 保存后台进程输出及异常，便于退出后排查。另以 `round-0001.json`、`round-0002.json` 等分别保存每次完成轮次：原文、源路径、实际原生按键及修饰状态、逐键时间戳、上屏归属、完整正文编辑记录、起止时间、采集异常与当轮参考快照。首次完成立即写入，参考仍在计算时记录 `pending`，完成或失败后原子更新同一轮；F3 重练另建文件，避免参考或实际序列被覆盖。目录权限 700，轮次文件权限 600，退出或翻页不会改写实际序列。日志随系统临时文件清理；长期仅缓存词库编译产物。`--plan-only` 将两种参考显示到标准输出。`--report-log` 读取完成轮次，按保存的本人序列和当轮参考重建对比，不启动输入法、不重新搜索、不伪造用时；缺失参考、粘贴或采集异常会保留原状态。

词库与公共 Rime 数据按内容哈希建立编译缓存，包含本地 YAML、Lua、OpenCC 内容及编译工具版本信息；内容不变时复用 `~/.cache/xkjd-type/dictionaries/`，变化后重新编译。缓存不是词库数据源。运行时从缓存创建可写隔离副本，真实输入法配置和词库只读，不部署到实际输入法。私有 D-Bus 禁用桌面服务自动激活，普通命令不注入按键、不读 `/dev/input`、不需要 root。

临时 Fcitx 输入法列表只放 Rime，临时 Rime 方案列表只放键道 6，默认从中文模式开始；跟打完成及重练均保持中文模式，不主动切到 Latin mode。这些约束仅作用于临时副本，参考计算也使用相同副本。

两种参考在后台由 A* 搜索 schema 字母表内完整已有的 ASCII 编码，并用真实 librime 回放候选及自动顶屏。首选空格、次选 `'`、三选 `3`，只允许完成已有编码后选重，不补全未完成前缀、不翻页。搜索按标点和换行拆成内部子问题，界面仍是完整入参内容；结果只声称限定范围内最优，不声称跨句段或所有操作的全局最优，也不做多音字语境判断。没有完整参考的英文等内容仍能练习，但不比较整篇效率。算法和统计不调用语言模型。

默认每种参考、每句段最多扩展 10000 个状态、搜索 45 秒，同时限制生成状态与 oracle 缓存。超限标为可行参考或不可用，不冒充已证明最优；可用 `--max-states`、`--search-seconds` 调整。后台计算可取消，退出或重练会终止其进程组。整个跟打进程树默认限制 2048 MiB、禁用交换空间、最多 64 个任务，可用 `--memory-mib` 调整。

码键指输入法处理的字母与引导键；删除字数按 Unicode 码点计数，独立修饰键与退出快捷键不计入总键。含粘贴、未完成、上屏无法关联到原生按键或采集异常时不比较效率。结果画面中的实际计数在完成时冻结，查看和滚动结果不会增加跟打键数。

首次运行按需编译原生桥接器。`xkjd-dict` 需已编译且在 PATH 中。以下测试不会注入桌面按键；采集回归测试会启动自己的临时 D-Bus：

```bash
python -m unittest discover -s tools/trainer -p 'test_*.py'
ruff check tools/trainer scripts/xkjd-type
pyright -p tools/trainer
```

`tools/trainer/desktop_smoke.py` 是显式桌面验证，会注入固定按键，须置于 KDE `ui_guard.py run --indicator` 下运行。测试专用临时状态随被测进程退出而清理。

## 许可证与私有数据

代码采用 [MIT License](LICENSE)。外部词库与参考数据不随代码授权；下载器记录各来源及其授权信息，缓存不纳入 Git 或 Cargo 发布包。

`.gitignore` 排除词库、外部缓存、输出报告、跟打日志及环境文件；不要强制加入这些产物。报告和日志可能包含正文、编码、文章路径和逐键时间，公开前须自行核对。Cargo 包使用文件白名单，仅包含 Rust 源码、测试、Rust 示例、README 和许可证；Python 实验工具由源码仓库提供。

提交前检查 `git status --short` 和拟提交 diff；`.gitignore` 不会移除已经跟踪的文件。桌面自动化额外依赖未随仓库提供的 KDE 工具，配置方法见 [采集实验说明](tools/key-capture/README.md)，不属于普通 CLI 或手动跟打所需依赖。
