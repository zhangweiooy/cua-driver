# Windows 真人示教接口

实现依据：`cua-pywin/docs/cua-driver-secondary-development-plan.md`（2026-10-13）。
本产品使用新的 CLI/MCP 接口；Oryx 负责界面、Agent 暂停、技能生成和长期数据管理。

## 调用

MCP 使用 `start_teaching`、`get_teaching_state`、`stop_teaching`。同一个运行时只允许一个活动示教；保持相同调用身份和 `session_id`。

```json
{"session_id":"demo-1","max_seconds":600,"output_dir":"D:/Demos"}
```

以上是 start_teaching 参数。output_dir 为可选的输出父目录；省略时使用产品用户数据目录下的 teaching 目录。每次新建 teaching-UUID 子目录，不覆盖已有会话。写入沿用 Driver 的文件输出授权边界。

```json
{"session_id":"demo-1","after_sequence":0,"timeline_limit":100,"after_evidence_sequence":0,"evidence_limit":100}
```

以上是 get_teaching_state 参数。动作与图片补充分别返回序号、有限数量的记录及文件引用。按 next_sequence 继续读取。只返回已完成记录，不事后修改已返回的条目。

```json
{"session_id":"demo-1"}
{"session_id":"demo-1","cancel":true}
```

以上分别是正常停止和取消。重复停止返回已保留的最终结果。取消只删除本会话实际创建的文件，保留其他会话及调用方另行放入的文件。

CLI 通过上游 `serve` 持久运行时调用。每次 JSON 中都提供同一个显式 `session` 标签；它是上游调用身份，session_id 是示教编号：

```powershell
# 在独立终端运行持久 Driver；正常权限模式保留上游授权流程
cua-win serve
# 以下调用使用不同 CLI 进程，但拥有同一命名会话
'{"session":"demo-client","session_id":"demo-1"}' | cua-win call start_teaching --json
'{"session":"demo-client","session_id":"demo-1"}' | cua-win call get_teaching_state --json
'{"session":"demo-client","session_id":"demo-1"}' | cua-win call stop_teaching --json
```

未指定显式 session 的一次性 CLI 启动请求会返回 teaching_requires_named_session，避免 CLI 退出立刻销毁示教。MCP 连接结束、显式结束所属会话、上游会话过期、600 秒保护时限或桌面不可访问都会结束采集并保留结果。

## 输出

一个会话目录包含：

- session.json：状态、原因、最终截止序号及丢失/失败计数。
- actions/000001.json 等：不可变动作 JSON。状态接口的 action_files 与返回动作对应。
- images/000001.png 等：必要截图。
- evidence/000001.json 等：窗口/PID、原事件时间、实际采集时间、图片几何、相对图片路径，以及按需 OCR 结果。
- complete.json：正常停止且动作采集、写入完整时生成，包含动作和证据截止序号及 evidence_complete。

顶层 complete 只表示动作采集及写入完整；evidence.complete 单独表示截图/OCR 补充证据完整。截图缺失、OCR 忙、补充超时或图片写入失败不会让已完整采集的动作变成失败；具体缺失原因保留在 evidence 中。动作丢失、文字无法确认、动作/状态写入失败或异常停止仍使 complete=false，不生成完成标记。

停止后文件直接可读，原路径继续有效。新会话不会清理旧目录。Oryx 自行展示、保留和删除；不需要复制完成确认、导出服务或历史数据库。不能保证崩溃前未写入的事件可恢复。

## 采集边界

Windows 钩子只记录输入、来源和时间，进入有界队列。独立处理路径进行文字/滚动/双击合并和简单 UIA 控件查询。慢 UIA 不在输入回调中运行；未知上下文保留元信息。

UIA 查询最多等待 750ms，与旧 cua-pywin 一致。点击按位置查询控件，不要求点击前后焦点相同，也不受 250ms 事件年龄限制；仍核对该位置属于原目标窗口和进程。键盘输入保留焦点和时效绑定，避免把其他控件的文字归入原输入。

控件提交值可读取时，拼音组合停顿不按固定时长判为文字缺失；Escape 取消且提交值未变化时不生成文字。读取 UIA 期间到达的按键先保留处理顺序，避免粘贴后的新值提前覆盖文字差分基准。

文字差分仅为同一窗口、进程和焦点控件的后续文字输入暂缓；点击、导航和其他控件的事件到来前，先结算已确认的文字。文字身份在点击、键盘和轮询路径中统一由目标进程、窗口及 UIA 控件信息生成；控件没有独立 HWND 时按有效的 0 处理，不混入宿主焦点 HWND。双击合并单独使用点击目标身份，不改变文字基准身份。

密码保护只使用 UIA 明确的密码属性。密码或无法确认的控件不保留文本和相关图片。不匹配敏感词，不扫描整个 UIA 树，也不作通用隐私识别保证。

每次事件只读取一次完整 UIA 上下文。截图前后只复核目标窗口/进程、控件 RuntimeId、密码属性和矩形，不再读取名称、角色或文本值；RuntimeId 无法读取时跳过该张图片，不影响已有动作。RuntimeId 同样支持没有独立 HWND 的控件。

点击和输入停顿可以保留对应控件区域的截图，绑定窗口、事件时间和实际采集时刻。停止不补拍旧事件。只有缺少控件提交文本时调用 OCR；结果是补充证据，不把拼音或 OCR 猜测直接当作已输入文字。显式视觉解析和示教复用同一上游 Perception 客户端、串行 worker、超时与空闲释放机制。

自动执行与示教仅在同一 Driver 运行时互斥：活动动作期间开始示教返回 teaching_busy；示教期间修改操作返回 teaching_active；只读、停止和必要清理允许执行。状态检查和操作进入使用同一个内存锁，覆盖竞态。不同运行时互不协调，不使用跨进程锁文件、注入标记或来源判断。

## 验证与来源

`verify-teaching.ps1 -Bundle <dist> -Python <development-python> -AllowDesktop` 运行相关单元、原生测试窗口、MCP 和持久 CLI 测试。普通 cargo test 不向桌面发送输入。真实输入法、真实业务应用和 Windows 1903 仍按本地方案验收。

钩子的有界入队和异步上下文处理参考已获授权的 Screenpipe：`b23efb5d876c04865e451155589783ad7911de4c`，`crates/screenpipe-a11y/src/platform/windows.rs`。保留 Screenpipe 及贡献者来源；其商业许可不因引用而改变。
