## ADDED Requirements

### Requirement: 自动增量索引配置

系统 SHALL 提供自动增量索引开关，新安装和没有显式配置的现有安装 MUST 默认开启；开关只控制运行期 watcher/coordinator，不得删除或禁用现有文件搜索索引。

#### Scenario: 自动增量默认开启

- **WHEN** QuickFox 创建默认配置或加载缺少 watcher 配置的旧配置
- **THEN** 自动增量索引开关为开启状态

#### Scenario: 用户关闭自动增量

- **WHEN** 用户在设置中关闭自动增量索引并保存
- **THEN** 系统停止运行期 watcher 和新事件消费
- **AND** baseline、journal 和当前文件搜索结果保持可用
- **AND** 用户仍可触发手动增量刷新

#### Scenario: 用户重新开启自动增量

- **WHEN** 用户重新开启自动增量索引并保存
- **THEN** 系统启动 watcher/coordinator
- **AND** 系统先对配置 root 执行增量校准，再声明自动增量已正常运行

#### Scenario: watcher 开关变化不全量重建

- **WHEN** 用户只修改自动增量索引开关而未改变索引语义配置
- **THEN** 系统不得仅因该开关变化触发全量索引重建
