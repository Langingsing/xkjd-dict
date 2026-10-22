# 终端输入捕获实验

此目录保留开发实验，不是普通跟打的安装步骤。`tty_probe.py` 与分析器可独立使用；三个桌面自动化入口（本目录的 `desktop_trial.py`、`private_fcitx_trial.py`，以及 `../trainer/desktop_smoke.py`）额外依赖仓库外的 KDE 自动化工具 `kde_ui.py` 和 `ui_guard.py`，本仓库不提供该工具。没有此依赖时可查看 `--help`，实际自动化会在操作桌面前明确拒绝。

如果已安装这套工具，将其脚本目录显式加入 `PYTHONPATH`，并用同目录的 guard 启动实验。下文的 `KDE_UI_SCRIPTS` 是使用者自行设置的目录；这不是正常手动跟打的依赖。所有桌面实验都必须在 guard 下运行：

```bash
export KDE_UI_SCRIPTS=/path/to/kde-ui-automation/scripts
export PYTHONPATH="$KDE_UI_SCRIPTS${PYTHONPATH:+:$PYTHONPATH}"
```

`tty_probe.py` 只观察自身终端 stdin，使用 raw 模式记录字节十六进制和读取时刻。读取批次不等于物理按键事件，也不能用其时刻冒充按键发生时刻。Ctrl+D、Ctrl+C 或超时会停止并恢复 termios；输出文件必须不存在，权限为 0600。

```bash
python tty_probe.py --output /path/to/new-session.jsonl --seconds 300
```

切英文直输一轮，回车保存样本；再用键道正常输入一轮，回车保存样本。探针只读 fcitx 状态，不自动切换输入法。`sample.display` 是终端字节解码，不能反推出被输入法消费的编码、选重键或退格。

`desktop_trial.py` 是单独的固定样例自动化实验。它通过 KDE 工具的临时 uinput 键盘发送 `hbkf`、空格、回车，分别测试英文与 Rime；若 Rime 仍传递英文，则尝试正常的 Shift 中英切换。发送序列是注入记录，不是捕获到的原生事件流。它应在 `ui_guard.py run --indicator` 下运行；产生 before.png 后暂停，检查测试窗口确实在前台，再在证据目录创建 `continue` 才发送。输出必须是新文件，证据目录必须为空。三个桌面自动化入口均要求 `--return-window` 指定清理后返回的窗口，不假定使用者打开了某个特定应用。本实验还恢复测试前的 fcitx 引擎与启用状态；不声称恢复了不可观测的所有 Rime 内部开关。

本机验证结果：KDE Wayland、Konsole、fcitx5/Rime 下，raw stdin 在英文直输时收到 `hbkf `，中文输入时只收到“回看”。普通 CLI 的 stdin 无法取得这次被输入法消费的编码及确认空格。当前用户直接打开实际键盘的 `/dev/input/event3`、`event4` 均得到 EACCES；没有修改设备权限。

上述结果是开发机器上的历史实验，不是跨环境保证，私人实验产物不随仓库提供。它只验证终端输入路径；完整跟打入口和支持环境见仓库 README。

## 输入法侧隔离实验

后续 `private_fcitx_trial.py` 已在私有 D-Bus 上运行独立 fcitx/Rime，并令测试 Konsole 的 `QT_IM_MODULE`、`QT_IM_MODULES` 同时为 `fcitx`。Qt 全局 `wayland;fcitx;ibus` 优先级会绕过私有服务，只设置前一个变量不够。这条路径不读取物理键盘设备，不改正在使用的输入法配置，仅接入受控测试子进程。

仅 fcitx 服务器使用隔离的 XDG 配置、数据和缓存目录；Konsole 客户端继承调用者原有目录，沿用用户终端的 profile、字体和配色，避免回退到默认字体。

`--work` 必须事先准备 `config/fcitx5/profile`（包含 keyboard-us 和 rime）、`data/fcitx5/rime/`（本地源词库和已编译的 xkjd6 schema）。测试副本可从本项目上一轮隔离编译目录复制。不要将 `--work` 指向实际输入法的配置根。运行依赖本机 fcitx5、Qt fcitx 插件、dbus-daemon、dbus-monitor、KDE 技能和可写的 /dev/uinput。

```bash
python3 "$KDE_UI_SCRIPTS/ui_guard.py" run --indicator \
  --activity '隔离输入法捕获实验' -- python private_fcitx_trial.py \
  --work /isolated/root --output /new/evidence/directory --return-window '返回窗口标题' \
  --selection-reference /path/to/verified-runtime-result.json
```

`--selection-reference` 可选；程序按编码长度、编码、正文顺序确定性选取已验证的次选与三选样例，追加到普通上屏、退格纠错两轮测试后。只在检查 before.png、确认测试窗口在前台并创建证据目录中的 `continue` 后发送固定按键。后台记录 `fcitx.log` 原始 key_trace、`commit-signals.log` 上屏信号、`tty.jsonl` 终端字节三条证据；注入清单独立保存，不当作捕获结果。测试必须逐项得到预期上屏，否则失败。退出时结束私有服务器、监听器和总线，回到指定窗口，不留下后台记录进程。

```bash
python analyze_fcitx.py --directory /evidence/directory --timezone Asia/Shanghai
```

分析器仅接受单个 Konsole 输入上下文：用回车分隔测试样本，将按下事件与上屏信号的墙钟时间关联，并逐项核对终端 UTF-8 内容。它检查按下/释放配对；如果有注入清单，还会逐键核对捕获完整性，但不使用清单生成按键数据。固定实验不支持自动重复按键。结果输出 capture.json 和 capture.md；缺少事件、格式未知、时钟倒退或证据不一致会失败。该原型没有正式归因候选序号、回删上屏正文或计算真人速度。

开发时的隔离记录包含普通输入 5 键、带一次退格纠错 7 键，次选和三选按键也已在真实 fcitx 路径观察到。这不表示对任意原生 Wayland 应用的全局捕获已经实现；原生终端仅靠 stdin 的限制仍然成立。自行运行时应保留三条证据核对结果，不将注入清单当作实际捕获。
