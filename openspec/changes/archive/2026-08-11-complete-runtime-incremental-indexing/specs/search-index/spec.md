## ADDED Requirements

### Requirement: 分层运行期搜索视图

系统 SHALL 以不可变 compact baseline、可变 delta overlay 和删除 tombstone 组成运行期文件搜索视图；普通增量批次 MUST 只重建 delta 候选结构，不得重建完整 baseline。

#### Scenario: 新增文件只进入增量层

- **WHEN** watcher 批次包含一个符合索引规则的新文件
- **THEN** 系统将该文件加入 delta overlay
- **AND** 系统不重建完整 compact baseline

#### Scenario: 修改条目覆盖基线条目

- **WHEN** delta overlay 包含与 baseline 相同规范化路径的新条目
- **THEN** 查询只使用 overlay 中的新条目
- **AND** 结果中不出现同一路径的 baseline 重复项

#### Scenario: 删除目录屏蔽整个基线子树

- **WHEN** tombstone 标记一个已删除目录
- **THEN** 查询在最终候选截断前屏蔽该目录和全部后代路径
- **AND** 其他 baseline 与 overlay 结果仍可参与排序

#### Scenario: 分层结果保持既有排序语义

- **WHEN** baseline 与 overlay 同时返回匹配候选
- **THEN** 系统使用现有 matcher 和 ranker 语义合并、去重和排序
- **AND** 应用、文件、目录类型优先级与历史权重保持有效

### Requirement: 增量 journal 与崩溃恢复

系统 SHALL 在切换内存增量视图前事务提交可幂等重放的 delta journal，并在启动时从最近可用 baseline 重放所有已提交且未合并的 journal batch。

#### Scenario: 启动重放已提交增量

- **WHEN** QuickFox 启动且存在 baseline 与未合并的 committed journal batch
- **THEN** 系统按 generation 顺序重放 journal
- **AND** 文件 Provider 在重放后使用恢复的 layered view

#### Scenario: 未提交批次不进入恢复视图

- **WHEN** 应用在 journal batch 提交前退出或崩溃
- **THEN** 重启时系统忽略该未提交批次
- **AND** 最近完成的 baseline 与已提交 journal 保持可用

#### Scenario: 重复重放保持幂等

- **WHEN** 同一个 committed journal batch 因恢复重试被再次应用
- **THEN** 系统按规范化路径得到与单次应用相同的 overlay 和 tombstone
- **AND** 不产生重复搜索结果

#### Scenario: journal 损坏时保留基线

- **WHEN** 系统无法解析或一致重放 journal
- **THEN** 系统保留最近可用 baseline 提供搜索
- **AND** 状态暴露 journal 恢复失败和后台全量刷新 fallback 原因

### Requirement: 目录清单增量校准

系统 SHALL 持久化已知目录的轻量指纹和父子关系，并在手动刷新或 dirty-root 恢复时只枚举指纹变化、缺失或新发现的目录。

#### Scenario: 未变化目录不重新枚举文件

- **WHEN** 手动增量刷新发现已知目录的指纹未变化
- **THEN** 系统不对该目录执行 `read_dir` 或重新构造其文件条目
- **AND** 系统继续检查清单中的已知子目录指纹

#### Scenario: 变化目录只比较直接子项

- **WHEN** 已知目录的指纹发生变化
- **THEN** 系统枚举该目录直接子项并与持久化清单比较
- **AND** 只为新增、变化和缺失路径生成 delta 操作

#### Scenario: 新目录递归建立清单

- **WHEN** 校准在变化目录下发现新的子目录
- **THEN** 系统按现有包含和排除规则扫描该子树
- **AND** 系统为新子树保存目录清单和索引条目

## MODIFIED Requirements

### Requirement: 手动刷新索引

系统 SHALL 提供手动增量刷新，并在部分目录失败时继续处理其他可用目录；手动刷新 MUST 优先提交待处理 watcher 事件并使用目录清单校准变化路径，只有索引语义配置改变、持久化状态无法恢复或校准无法建立可信差异时才升级为后台全量重建。

#### Scenario: 手动刷新默认执行增量校准

- **WHEN** 用户触发手动刷新且 baseline、journal 和目录清单可用
- **THEN** 系统先提交待处理事件并校准 dirty 或变化目录
- **AND** 后续搜索结果反映新增、删除或修改路径
- **AND** 系统不重新构造明确未变化的文件条目

#### Scenario: 索引语义变化触发全量重建

- **WHEN** 索引包含目录、排除目录、排除模式、项目 ignore 或内容索引范围改变
- **THEN** 系统启动后台全量重建
- **AND** 状态明确显示全量重建及触发原因

#### Scenario: 持久化状态异常触发全量重建

- **WHEN** journal、目录清单或 schema 无法恢复到可信增量状态
- **THEN** 系统保留最近可用 baseline
- **AND** 系统在后台全量重建索引
- **AND** 状态明确显示 fallback 原因

#### Scenario: 部分目录失败不阻塞刷新

- **WHEN** 增量校准中某个目录因权限或不存在而失败
- **THEN** 系统报告该目录失败并继续处理其他目录
- **AND** 未确认删除的 baseline 条目保持可用

### Requirement: 持久化索引快照

系统 SHALL 将成功构建的文件索引 baseline 持久化到本地存储，并将后续运行期变化保存为可重放 journal；下次启动 MUST 先加载最近完成的 baseline，再重放所有已提交且未合并的 journal batch。

#### Scenario: 启动加载基线与增量

- **WHEN** QuickFox 启动且存在最近完成的 baseline 与 committed journal
- **THEN** 文件 Provider 使用 baseline 和重放后的增量视图提供搜索结果
- **AND** 自动增量准备或后台刷新不阻塞非文件 Provider

#### Scenario: 旧快照兼容迁移

- **WHEN** QuickFox 升级后只存在旧格式完整快照
- **THEN** 系统加载旧快照作为 baseline 提供搜索
- **AND** 系统在后台建立目录清单和新的增量状态

#### Scenario: 无快照时文件搜索不可用

- **WHEN** QuickFox 启动且没有任何完成的索引 baseline
- **THEN** 文件 Provider 不阻塞查询，并向前端暴露文件搜索暂不可用的状态

### Requirement: 运行期文件系统监听

系统 SHALL 在应用运行期间监听已索引根目录变化，通过有界队列和 debounce 批量更新受影响路径；普通事件从 watcher 到达后 MUST 在 10 秒内进入可搜索视图，平台失败或事件溢出 MUST 转为 dirty-root 校准而不是静默丢失一致性。

#### Scenario: 文件创建后进入索引

- **WHEN** 用户在已监听根目录下创建符合索引规则的文件
- **THEN** watcher 将事件发送到有界批处理队列
- **AND** 系统在事件到达后 10 秒内更新该路径的 name/path 索引
- **AND** 若文件符合内容索引范围和大小限制，系统独立更新内容索引

#### Scenario: 文件删除后从索引移除

- **WHEN** 用户删除已索引文件或目录
- **THEN** 系统在事件到达后 10 秒内使用 tombstone 从搜索视图移除对应路径或子树
- **AND** 系统将删除操作写入 committed journal

#### Scenario: 文件重命名原子折叠

- **WHEN** 平台 watcher 报告文件或目录重命名
- **THEN** batcher 将其折叠为旧路径 tombstone 与新路径 targeted scan
- **AND** 查询不同时显示重命名前后的重复结果

#### Scenario: 事件风暴转为 dirty-root 校准

- **WHEN** 平台报告 overflow 或有界 channel 无法接收更多事件
- **THEN** 系统将可识别的受影响 root 标记为 dirty
- **AND** 系统安排目录清单校准或带原因的后台全量刷新
- **AND** 最近可用搜索视图保持可用

#### Scenario: watcher 失败时降级

- **WHEN** 平台 watcher 初始化或运行失败
- **THEN** 系统暴露结构化失败 code 与用户可读摘要
- **AND** 系统回退到手动增量刷新或后台刷新
- **AND** 启动器基础搜索能力仍保持可用
