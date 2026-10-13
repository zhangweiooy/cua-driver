# 共享真人示教接口

共享核心负责会话、事件处理、证据与文件；Windows/macOS 后端负责原生监听、上下文和截图。Windows 基线及 macOS 验收边界分别见平台说明。
本产品使用新的 CLI/MCP 接口；Oryx 负责界面、Agent 暂停、技能生成和长期数据管理。

## 调用

MCP 使用 `start_teaching`、`get_teaching_state`、`stop_teaching`。同一个运行时只允许一个活动示教；保持相同调用身份和 `session_id`。

```json
{"session_id":"demo-1","max_seconds":600}
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

以下是与平台无关的命令形式；将 `<driver>` 替换为 Windows 的 `cua-win.exe` 或 macOS 的 `Cua.app/Contents/MacOS/cua`，JSON 通过标准输入传入：

```text
<driver> serve
<driver> call start_teaching --json     {"session":"demo-client","session_id":"demo-1"}
<driver> call get_teaching_state --json {"session":"demo-client","session_id":"demo-1"}
<driver> call stop_teaching --json      {"session":"demo-client","session_id":"demo-1"}
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

平台后端把原生输入转换为语义按键、点击、滚动事件，进入有界队列。共享层合并文字、滚动和双击。UIA/AX 查询不在原生输入回调中运行；未知上下文保留元信息。

后端上下文查询采用有界工作线程，最多等待 750ms。点击查询点击目标，键盘查询焦点目标；时效与身份由后端核验，核心只比较不透明的窗口/控件标识。Windows 的 HWND、虚拟键码和 UIA RuntimeId 不作为 macOS 或公共协议的前置条件。键盘动作以 keyName/modifiers 表达；Windows 保留既有可选 keyCode 诊断字段，消费者不能依赖它跨平台回放。

控件提交值可读取时，拼音组合停顿不按固定时长判为文字缺失；Escape 取消且提交值未变化时不生成文字。读取原生上下文期间到达的按键先保留处理顺序，避免粘贴后的新值提前覆盖文字差分基准。

文字差分仅为同一窗口、进程和焦点控件的后续文字输入暂缓；点击、导航和其他控件的事件到来前，先结算已确认的文字。文字身份与点击合并身份分别处理，不因第一次点击改变焦点而拆散双击。Windows 保留已验收的无独立 HWND 控件处理；macOS 用 AX 远程元素身份。

密码保护只使用系统明确的安全字段属性（Windows UIA IsPassword；macOS AXSecureTextField）。密码或无法确认的控件不保留文本和相关图片。不匹配敏感词，不扫描整个树，不作通用隐私识别保证。

截图前后只复核目标身份、密码属性及必要几何，不重复读名称或文本。截图凭据留在原生适配层；共享层调度截图/OCR、控制队列和停止截止点。控件身份无法确认时跳过图片，不影响已有动作。

点击和输入停顿可以保留对应控件区域的截图，绑定窗口、事件时间和实际采集时刻。停止不补拍旧事件。只有缺少控件提交文本时调用 OCR；结果是补充证据，不把拼音或 OCR 猜测直接当作已输入文字。显式视觉解析和示教复用同一上游 Perception 客户端、串行 worker、超时与空闲释放机制。

自动执行与示教仅在同一 Driver 运行时互斥：活动动作期间开始示教返回 teaching_busy；示教期间修改操作返回 teaching_active；只读、停止和必要清理允许执行。状态检查和操作进入使用同一个内存锁，覆盖竞态。不同运行时互不协调，不使用跨进程锁文件、注入标记或来源判断。

## 验证与来源

公共回归位于 `cua-driver-core::teaching`；Windows 原生验收通过 `windows/verify-teaching.ps1` 执行。平台构建、权限和原生验证清单分别见 `windows/README.md`、`macos/README.md`。macOS 实际运行尚待可用 Mac，不以共享测试通过替代。

Windows 实现原参考 Screenpipe `b23efb5d876c04865e451155589783ad7911de4c` 的输入钩子处理；此次 macOS 参考的固定版本及吸收范围见 macOS 说明。用户已授权使用。保留原项目及贡献者来源，不将授权等同于更改上游许可证。
