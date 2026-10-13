# 真实输入法验收 — 2026-10-13

## dev.10 无独立 HWND 控件身份修复

修复 dev.9 把点击身份复用到文字路径造成的回归。键盘、轮询、点击现在使用同一规则生成文字控件身份：进程、窗口、UIA 原生句柄及既有控件属性。NativeWindowHandle 为 0 时不再仅在键盘路径替换成宿主焦点 HWND。[微软文档确认无独立窗口的控件返回 0](https://learn.microsoft.com/en-us/previous-versions/dd319225(v=vs.85))。

`Context.click_target` 单独用于点击合并和点击图片目标校验；文字基准只使用统一的 `Context.focus`。因此修复文字连续性不会撤销双击修复。共享宿主、NativeWindowHandle 同为 0 的不同控件仍由各自的 UIA 控件信息区分。

新增回归先在原身份规则下复现 `"a"` 未输出且 `text_unavailable=1`，修复后通过。覆盖无 HWND 控件的键盘→点击→轮询→继续输入、共享宿主的不同虚拟控件、点击合并与文字身份相互独立。前两轮的跨控件结算、粘贴、IME 提交、双击回归继续通过。280 项 Windows 单元测试与 4 项原生示教测试通过；最终包 CLI/MCP、互斥/停止/文件输出及完整性检查（含 9 个负向用例）通过。

另用 `tests/HwndlessFixture.cs` 创建真实 WPF TextBox。夹具在独立线程经 UIA Client 查询自身控件，实际得到 `a_hwnd=0`、`b_hwnd=0`，宿主 HWND 非零；没有把 0 写死为验证结果。经 Computer Use 输入 `a`，点击同一输入框，再输入 `b`，双击切换到 B 后输入 `c`。最终 A=`ab`、B=`c`，三个文字动作分别归属 A/A/B，B 的点击为 `count=2`。最终 8 个动作、6 张可读截图，所有缺失/丢弃/输出错误计数为 0，`complete=true`。精确的“尚待提交时点击”时序由确定性回归覆盖；WPF 测试补充真实无 HWND 提供程序上的交互验证，不冒充物理键盘测试。

夹具只用于开发验收，不进入运行目录或安装包。可用 .NET Framework 4 的 `csc.exe /target:winexe` 编译，引用 WPF 的 `PresentationFramework`、`PresentationCore`、`WindowsBase`、`UIAutomationClient`、`UIAutomationTypes`，以及 `System.Xaml`、`System.Web.Extensions`；启动参数是测试状态 JSON 的输出路径。输入由 Computer Use 发送，夹具不注入输入。

证据：`.cache/tests/hwndless-before.log`、`hwndless-windows.log`、`hwndless-native.log`、`hwndless-dev10/04-final.json`、`hwndless-dev10/wpf-final.json`、`hwndless-dev10/hwndless-acceptance.json`。会话目录：`sessions/teaching-149244e8-d86e-49eb-aca8-9c1acd80e5c6/`。测试窗口与运行时已退出。

安装包：`out/cua-win-0.4.0-dev.10-win64-setup.exe`，94,034,744 字节。
SHA-256：`7b312b7a7777bfe9cad25566af9e0604d811b4b8d04ad34468f4883305ad2cc0`。
对应源码：`out/source/cua-win-0.4.0-dev.10/`。代码未提交。

## dev.9 审查问题修复及复验

本轮修复两项审查发现。均先添加回归，在未修改生产逻辑时复现失败，再完成修复和复验。

- **P1：已确认文字在控件切换前结算。** `TextCapture::observe` 现在接收下一条实际事件，核对窗口、进程、焦点及事件类型。只为当前控件的文字延续/提交延迟处理；点击、导航及其他来源不再阻止结算已读取的文字。没有待提交文字时仍为修饰键前缀保留粘贴基准，原粘贴及 IME 提交时序回归继续通过。
- **P2：双击身份来自目标控件。** 点击上下文使用被点击控件的原生句柄与既有控件属性，不再把点击前的焦点 HWND 作为目标身份。虚拟 UIA 控件没有 HWND 时沿用既有语义/几何字段；键盘输入的焦点绑定不变。

P1 的确定性回归覆盖 A 的“a”已读取、下一事件为点击、其他焦点/窗口/进程的输入、Tab、Ctrl+A 或滚动的情况，验证文字先输出且 `text_unavailable=0`。P2 的原生回归保留真实 UIA 查询，构造同一目标、前置焦点不同、间隔 100ms 的两次点击：修复前两条 `count=1`，修复后一条 `count=2`。

最终 dev.9 运行目录再经 Computer Use 实际双击 A、B 两个尚未获焦点的输入框，得到两条分别绑定 A/B 的 `count=2` 记录、4 张可读截图，所有诊断计数为 0，`complete=true`，完成标记及文件引用有效。此桌面复验验证双击；快速输入切换的精确队列时序由上述确定性回归验证，不冒充物理键盘测量。

验证结果：277 项 Windows 单元测试、4 项原生示教测试、最终包 CLI/MCP、互斥/停止/文件输出及包完整性（含 9 个负向用例）通过。之前的长时间选词、取消组合、快速粘贴和提交键去重回归继续通过。

修复前日志：`.cache/tests/review-boundary-before.log`、`review-double-before.log`。
修复后日志：`.cache/tests/review-boundary-after.log`、`review-native-after.log`、`review-fix-*.log`。
桌面证据：`.cache/tests/review-dev9/04-final.json`、`double-click-acceptance.json`，会话 `sessions/teaching-45d10609-29d7-4b5d-936b-f4ec6fa0f77f/`。测试窗口和运行时已关闭。

安装包：`out/cua-win-0.4.0-dev.9-win64-setup.exe`，94,034,452 字节。
SHA-256：`2cc734caabbb78b1d7310907baea7159d20bb1c62c75aa3a9341ab993ccfc18b`。
对应源码：`out/source/cua-win-0.4.0-dev.9/`。代码未提交。

## dev.8 点击限制对齐及复验

**此前的点击上下文降级已在同一临时双输入框场景中消除。** UIA 等待上限由 100ms 调整为旧 cua-pywin 的 750ms。点击按坐标确定目标，不再要求点击前后焦点相同，也不再应用 250ms 事件年龄限制。位置仍须属于原窗口和进程，密码属性仍先于内容读取；键盘输入仍保留焦点和时效校验。

本机通过 Computer Use 实际点击 A 输入框，再点击切换到 B，并通过 Microsoft 拼音提交“你”。两次点击均取得正确的控件名称与“编辑”角色，保存对应截图；后续中文正常采集。最终 3 个动作、3 张可读 PNG，所有诊断计数为 0，`complete=true`，完成标记及动作/证据截止序号均为 3。

新增原生回归覆盖点击后焦点变化、超过 250ms 的点击、错窗口/错进程拒绝，以及密码控件保护；原来的“仅切换前台即拒绝”截图负向用例改为真实错目标窗口，避免把允许的行为继续当成错误。276 项 Windows 单元测试、3 项原生示教测试、最终包的 MCP/持久 CLI 与包完整性（含 9 个负向用例）全部通过。测试窗口与测试运行时已退出。

证据：`.cache/tests/click-dev8/05-final.json`、`click-acceptance.json` 以及 `sessions/teaching-c4790eb8-1051-4a60-bc84-742525b6faec/`。
安装包：`out/cua-win-0.4.0-dev.8-win64-setup.exe`，94,035,139 字节。
SHA-256：`a0cb5063456906b1dc412a43eb27939fcd5528f5f5d5b4322fd252419a0da44a`。
对应源码：`out/source/cua-win-0.4.0-dev.8/`。代码未提交。本次通过不扩展为物理键盘、Windows 1903 或所有业务应用验收。

## dev.7 修复及复验

**两项已报告的文字采集缺陷已修复并通过本机 Microsoft 拼音复验。** 固定上游版本不变，没有新增跨进程协调或输入法框架。

- 可读取控件的提交值未变化时，保留待提交状态，不再用固定 1500ms 期限判定文字缺失；Escape 取消且提交值未变时清除待提交状态。真正无法读取的控件仍保留缺失/OCR 降级路径。
- UIA 读取期间若有输入进入队列，先保留事件顺序，暂不消费差分或推进基准文本；这样既能记录快速粘贴，也不会把晚处理的输入法提交键误当作新一段文字。结束时未处理的预读事件计入丢失数量。
- 改动位于 `platform-windows/src/teaching.rs` 的 `TextCapture::observe` 和事件循环的一条预读记录；复用原有基准、待提交状态与有界队列。

实际复验：输入 `n` 后保持候选状态超过 1.5 秒，再输入 `i`、空格提交“你”；A 控件提交文字只记录一次。将 A 的“你”复制并粘贴到 B，记录包含 B 的 `typeText:你` 和 Ctrl+V。B 再输入 `n`、停顿后 Escape，未产生虚假文字。停止后 `text_unavailable=0`、`dropped_events=0`、`output_errors=0`；7 个动作文件与接口一致，2 张 PNG 可读。

切换输入框时仍出现一次点击的 UIA 上下文读取不可用，`context_unavailable=1`，故整段 `complete=false` 且不生成完成标记。这是 dev.6 已出现的点击上下文降级，并未通过清零计数或放宽密码保护隐藏它。此次结论仅覆盖所修复的两项文字问题，不宣称所有示教边界已通过。

验证：276 项 Windows 单元测试通过（含 5 个针对性时序/组合回归）；2 项明确启用的原生示教测试通过；最终包的 MCP、持久 CLI、互斥、停止/断连、文件输出/取消，以及包完整性和 9 个负向用例通过。Rust 格式及差异空白检查通过。未重复执行未改动的全部 core/contract 测试。

本轮仍使用 Computer Use 自动按键经过真实系统输入法，不代表物理键盘真人操作验收。检查点与最终证据位于 `.cache/tests/real-ime-dev7/`；会话为 `sessions/teaching-88e5614c-1941-438a-a7e5-3b2aac44a6b5/`。测试窗口与运行时已关闭。

安装包：`out/cua-win-0.4.0-dev.7-win64-setup.exe`，94,037,298 字节。
SHA-256：`b34365c539d1001c34f34a21adf8ee5a1ac0554c720fd708c7de5a35c1eb7271`。
对应源码：`out/source/cua-win-0.4.0-dev.7/`。代码未提交。

## dev.6 原始验收记录

以下保留修复前的复现证据，代码行号对应当时的 dev.6 源码。

结论：**未完全通过**。中文提交可以正确采集，但组合阶段的缺失计数和粘贴文字采集存在问题。本轮不修改生产逻辑，不重新打包。

候选为 `cua-win 0.4.0-dev.6`；Windows 11 26H2，Build 26300.9457；本机 Microsoft Pinyin，HKL `0x8040804`。在两个原生 Win32 EDIT 控件中，通过 Computer Use 的按键接口发送拼音、空格、数字候选键和快捷键。实际观察到了系统候选窗口；没有通过直接写入中文或 SetValue 代替输入法提交。此结果属于真实输入法自动交互验收，未包含物理键盘真人操作或所有业务应用。

## 结果

| 场景 | 控件实际结果 | 示教结果 |
| --- | --- | --- |
| `nihao` + 空格 | A=`你好` | 通过，动作 2 为 `typeText:你好` |
| `ni` + 数字 2 | A=`你好尼` | 通过，动作 3 为 `typeText:尼` |
| 输入 `n` 后 Escape | A 仍为 `你好尼` | 未记录虚假的拼音文字，Escape 已记录；但组合停顿误增缺失计数 |
| Ctrl+A，输入 `h`，选第 2 候选 | A=`好` | 通过，动作 5 为 Ctrl+A，动作 6 为 `typeText:好` |
| 复制 A，切换 B，Ctrl+V | B=`好` | **失败**：动作 10 只有 Ctrl+V，没有 B 控件的 `typeText:好` |
| Shift 切到英文，输入 `abc` | B=`好abc` | 通过，动作 11–13 分别为 a、b、c |
| 恢复中文，B 输入 `n` + 空格 | B=`好abc你` | 通过，动作 14 为 `typeText:你`，控件来源为 B |
| 正常停止、文件读取 | 截止序号 14 | 通过，14 个动作 JSON、9 个证据 JSON 与接口记录一致，7 张 PNG 可读；部分记录未生成完成标记 |

## 已复现问题

1. **组合停顿被误报为文字缺失。** 拼音仍在候选窗口、EDIT 的提交值尚未变化时，`text_unavailable` 已增长。`03-commit-nihao.json` 中“你好”已经正确记录，该计数仍为 5。当前 `platform-windows/src/teaching.rs:658` 对待提交文字采用固定 1500ms 期限，超时直接清空并增加缺失计数，没有区分仍在组合的输入法状态。慢速选词、取消组合会造成不必要的部分记录判定。
2. **粘贴内容漏采集。** `07-paste-field-b.json` 的原生控件读回值是 `好`，最终动作只有 Ctrl+V；后续英文和中文能够继续被采集。相关路径为 `platform-windows/src/teaching.rs:623` 的按键挂起与 `:632` 的 UIA 基线差分。具体时序原因尚未确定，不能以快捷键已记录视为粘贴文本验收通过。

最终诊断：`text_unavailable=11`、`context_unavailable=2`、`dropped_events=0`、`output_errors=0`。两次点击的上下文补充不可用，已保留坐标并跳过图片；正常文字提交的 7 张图片均已保存。最终 `active=false`、`termination_reason=requested`、`complete=false`；输出正确暴露部分状态，没有宣称完整成功。

## 复现与证据

- 测试夹具：`ime_fixture.py`，仅创建窗口、读回控件及调用示教接口，不注入输入。
- 命令：`python products/cua-win/ime_fixture.py --bundle products/cua-win/out/cua-win-0.4.0-dev.6.dist --output <新的测试目录> --allow-desktop`。
- 在测试目录写入 `command.json`，例如 `{"id":"01-start","action":"start"}`，再经 UI 输入；`state` 保存检查点，`stop` 保存最终记录，`quit` 关闭夹具及测试运行时。
- 本轮检查点与汇总：`.cache/tests/real-ime-20261013-b/` 中的 `01-start.json` 至 `10-final.json`、`ime-acceptance.json`。
- 会话文件：该目录下 `sessions/teaching-69413117-d0e5-462e-ba8b-21b2c3087540/`。

测试窗口及示教运行时已结束，证据文件保留。夹具通过 Python 编译检查；未改动安装包或用户业务应用。
