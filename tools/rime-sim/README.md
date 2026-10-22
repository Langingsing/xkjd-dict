# Rime 逐键模拟

`engine.cpp` 使用本机 librime 和 librime-lua 的真实候选及 `process_key` 上屏结果，通过 JSON Lines 接收请求。每个请求创建独立会话，不调用强制上屏 API；不依赖 LLM。只向隔离副本写入 Rime 运行数据。

编译（需要本机 rime、nlohmann-json 开发头文件）：

```bash
g++ -std=c++17 -O2 engine.cpp -o /path/to/engine $(pkg-config --cflags --libs rime) -ldl
```

JSON 请求示例：

```json
{"schema":"xkjd6","keys":[104,98,32],"trace":true}
```

`keys` 是按键整数码，ASCII 按键使用其 ASCII 码。返回最终上屏文本、未上屏输入、当前页候选及可选的逐键记录。未被处理的可打印 ASCII 按客户端直通计入输出。`single:true` 开启 `danzi_mode`，不会改动源 schema。程序加载 `/usr/lib/rime-plugins/librime-lua.so`，使用 `/usr/share/rime-data` 作为共享数据目录；这是针对本机环境的分析工具。

`simulate.py --help` 提供批量入口，输入上一轮抽样的 `samples.json`。源编码通过既有 `xkjd-dict query` 读取，不自行解析 dict。对出现在样本中的字词，只尝试其已有的完整编码（包含已有简码），只取首屏前三候选：首选用空格，次选用 `'`，三选用 `3`。禁止数字 `2`、`4–9` 选重，也不翻页。即使补全候选已经出现，只要输入还不是该目标的一条已有编码，就不能确认或选重。自动上屏同样要求输入是目标的已有编码。所有按键都送入真实引擎验证，不能假定 `'` 映射必然生效。

逐句用动态规划选择最少显式按键路径，再逐个尝试删掉空格，只有全句回放完全匹配才保留删除。每个标点分隔的句段独立回放，标点两边都不计数；不跨标点拼词。dz 强制只用单字边。

```bash
python simulate.py --engine /path/to/engine --user /isolated/rime \
  --samples /path/to/samples.json --output /path/to/report
```

运行前必须在隔离副本编译所需 schema。工具不会替你修正源配置。如果副本为模拟所作的修正，须在报告中单独披露；不能把修正后结果称为原配置结果。缺失可达路径会在 JSON 中列出，不按零键计入有效对比。

源编码索引也读取 `--user` 指定的隔离副本，避免运行中实际词库变化导致索引与已编译的 Rime 表不一致。`report.py --help` 提供逐键记录核对及 Markdown/JSON 报告生成入口。

报告读取实际的 `xkjd6dz.custom.yaml` 并记录其内容；可用 `--baseline-probe /path/to/probe.json` 附上原配置探针。未提供探针时，报告不对原配置候选可达性作结论。报告生成前会核对样本数、编号、源路径、行号、完整编码、选重键、逐键记录和目标文本，并拒绝不符合当前选重规则的旧结果。

计数分开统计编码、空格、次选 `'`、三选 `3`。编码允许现有的多音及助记码，不做语音或语义消歧。需要 Python 的 PyYAML 来读取编译后的 schema。搜索覆盖 schema 的 `speller/alphabet` 中的 ASCII 编码和首屏前三候选，不覆盖所有引导码、退格改码或快捷键；删空格也采用贪心法，所以是可回放的搜索结果，不保证全局最优。独立干净会话不代表桌面当前开关、历史记录或动态学习状态；默认明确设置中文、简体，关闭 emoji，其他选项按 schema 默认值。
