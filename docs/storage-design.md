# SQLite 与本地文件存储设计

本文是 rereader 已实现的存储模块说明，入口为 `src/storage/mod.rs`，配套 [v1 建表 SQL](storage/001_initial.sql)、[v2 远端注释快照迁移](storage/002_annotation_conflicts.sql) 与 [v3 书架分组迁移](storage/003_shelf_groups.sql)。当前会话、书架和封面已接入；章节、进度、注释、设置与同步队列提供 Rust API，正文和笔记 UI、远端写回仍待业务接入。

## 1. 数据存储边界

以以下划分为准，不把正文或图片 BLOB 放入 SQLite：

| 数据 | 存储方式 | 对应结构 |
| --- | --- | --- |
| 书籍 ID、名称、作者 | SQLite | `books` |
| 书架、阅读进度 | SQLite | `shelf_entries`、`shelf_groups`、`reading_positions` |
| 笔记、书签、划线 | SQLite | `annotations`，按 `kind` 区分 |
| 网络同步状态 | SQLite | `sync_jobs`、`sync_cursors`、`reading_intervals` |
| 书籍正文及章节内容 | 本地文件，SQLite 存索引 | `chapters`、`cached_files` |
| 封面、插图 | 本地文件，SQLite 存路径 | `cached_files` |
| 账号、当前会话、阅读设置 | SQLite | `accounts`、`sessions`、`reader_settings` |

采用一个数据库、多账号隔离。所有业务主键和查询都包含 `account_id`，其值为微信读书 `vid`；每个账号分别保存书籍元数据与文件，避免把其他账号的正文权限或缓存混用。当前只允许一个活动会话。

## 2. 目录结构

```text
$REREADER_DATA_DIR/              # 沿用 Store::default_path() 的目录选择
├── rereader.sqlite3            # 元数据、用户数据、任务与索引
├── content/<account_hash>/
│   ├── book/<file_id>.bin    # 整书文件，media_type 标注实际格式
│   └── chapter/<file_id>.bin # TXT/HTML 等；解析后的版本另分配 file_id
├── images/<account_hash>/
│   ├── cover/<file_id>.bin
│   └── illustration/<file_id>.bin
└── tmp/<random_id>.part
```

`account_hash` 使用账号 ID 的 SHA-256；`file_id` 使用 128 位随机标识；文件扩展名统一 `.bin`，真实格式由 `media_type` 指定。远端 `bookId`、`chapterUid` 和 URL 不直接拼文件路径。数据库只存相对数据根目录的路径，方便整体迁移。

路径读取时必须拒绝绝对路径、`..`、符号链接逃逸，确认最终路径位于数据根目录内。目录 `0700`，数据库、临时文件及其他私有文件 `0600`；数据库创建前设置权限策略，覆盖 journal 或未来 WAL 附属文件。SQLite 本身不加密 token，日志、调试输出和导出文件不得包含凭据。

## 3. 统一字段约定

- 云端标识使用 `TEXT`，包括 `book_id`、`chapter_uid`、`remote_id`；它们是不透明标识，不假设永远能转整数。
- `*_at_ms` 使用 UTC Unix 毫秒 `INTEGER`；远端秒时间戳由协议层显式转换。缺失远端时间、版本、游标用 `NULL`，不伪造为当前时间或 `0`。
- 布尔使用 `INTEGER CHECK (... IN (0,1))`。本地 `revision` 递增，远端 `remote_revision` 保持原样。
- JSON 仅用于尚未稳定或结构多变的定位、样式、任务快照和设置；账号、书籍、排序、同步状态等可查询字段独立建列。Rust 使用带 `schema_version` 的类型验证 JSON，SQL 不依赖 JSON 扩展。
- 用户数据不能因移出书架、清目录或清缓存被级联删除。只有明确删除整本书本地数据或删除账号时才删除 `books` / `accounts`。

## 4. 表结构与关系

### 4.1 账号和会话

`accounts(account_id)` 保存身份和 `auth_epoch`；`sessions(singleton=1)` 保存当前账号的 `access_token`、`refresh_token`、`device_id`。不保存二维码、授权 code 或临时阅读签名。

登录、重新登录和登出在事务中递增对应账号的 `auth_epoch`。异步操作捕获 `(account_id, auth_epoch)`，写回前在同一事务中确认仍与活动会话一致，防止退出后或同账号重新登录后旧请求写回。普通 token 刷新沿用 epoch，由一个工作线程串行执行；轮换的两个 token 必须同时提交，之后才重试网络请求。

登出删除 `sessions`、递增 epoch 并停止账号同步，保留书架、进度和笔记以供以后登录恢复；未登录界面不能读取这些账号数据。“清除账号数据”单独删除 `accounts` 并清理账号文件。SQL 级联只删除文件索引，物理文件由存储模块回收。

### 4.2 书籍、书架和目录

| 表 | 主键 | 关键字段与含义 |
| --- | --- | --- |
| `books` | `(account_id, book_id)` | `title`、`author`、`cover_url`、`description`、`content_version`、`format` |
| `shelf_entries` | `(account_id, book_id)` | `sort_order` 为服务器列表顺序；`finished` 是账号的读完标记；`last_read_at_ms` 支持最近阅读排序 |
| `chapters` | `(account_id, book_id, chapter_uid)` | `ordinal` 是目录显示顺序，不能用 UID 排序；`parent_uid` 支持卷标题；`readable` 区分可读章节 |

`Book { id, title, author, cover, finished }` 对应 `books` 与 `shelf_entries` 的 JOIN，现有 UI 数据类型无需绑定数据库连接。

完整书架响应成功解析后，在一个事务中 upsert 书籍、替换当前账号的 `shelf_entries` 并更新同步游标。只有协议确认是完整快照时才能以“缺失项”删除书架关系；增量响应必须按服务端增删语义应用。移出书架不删除 `books`、笔记、位置或离线文件。

`shelf_groups(account_id, group_id)` 保存微信读书 `archive[]` 的名称、顺序和书籍 ID 列表（`book_ids_json` 为原始成员列表）。未知书籍 ID 也保留，界面计数和封面只使用当前电子书书架内的成员。`load_shelf_snapshot` 在同一事务中读取书籍和分组，`apply_shelf_snapshot_with_groups` 在同一事务中替换两者及游标，防止分组与书架错配。旧的 `apply_shelf_snapshot` 表示无分组的完整快照，会清除该账号旧分组。

目录完整快照可事务替换；增量目录按 UID 合并及处理明确删除项。位置、笔记、缓存只保留逻辑 `chapter_uid`，不对易被替换的目录行建立删除级联；目录更新后发现章节不存在或版本改变，交给定位恢复逻辑处理。

### 4.3 文件索引

`cached_files(account_id, file_id)` 统一索引正文与图片：

| 字段 | 用途 |
| --- | --- |
| `kind` | `book` 整书、`chapter` 章节、`cover` 封面、`illustration` 插图 |
| `book_id` / `chapter_uid` | 所属书籍和可选章节；章节正文必须带 UID |
| `resource_key` | 逻辑资源标识，例如正文章节键、封面 URL 摘要、HTML 内资源键 |
| `content_version` | 服务端版本，缺失时保留 NULL；同一资源允许保留多个本地版本 |
| `variant` | `original` 或 `gray-200x300-v1` 等处理结果，避免原图与灰度缩略图混用 |
| `relative_path` | 正式文件的唯一相对路径 |
| `media_type` / `byte_size` / `sha256` | 格式、大小、实际字节校验值 |
| `pinned` | 用户指定保留的离线内容，不参加自动 LRU 清理 |
| `last_accessed_at_ms` | 淘汰顺序；按批次更新，避免每次图片访问都写数据库 |

不把下载中的 `.part` 当成可读正文。`source_url` 仅允许无敏感参数的稳定地址；短期签名 URL 留在内存，资源重新请求时再获取。文件缺失、校验失败或版本不符时视为缓存未命中，清理索引后重新下载。`pinned` 只表示本地保留策略，读取权限仍由业务层依据已验证协议判断。

### 4.4 阅读位置

`reading_positions` 每本书最多两行：`origin=local` 是本机最后阅读位置，`origin=remote` 是最近一次读取的云端位置。下载云端进度不能直接覆盖尚未同步的本地位置。

`locator_json` 的建议格式如下，具体协议偏移仍需联调确认：

```json
{
  "schema_version": 1,
  "render_anchor": {
    "unit": "unicode_scalar",
    "offset": 1280,
    "text_sha256": "<解析后的章节文本摘要>"
  },
  "source_anchor": null,
  "range_end": null
}
```

确认墨水屏协议后，才填 `source_anchor`，例如显式标注 `unit`、原文 `offset`、`protocol` 与版本。不能把处理后文本下标、UTF-8 字节、UTF-16 下标混为同一个 `offset`。`chapter_uid` 和 `content_version` 在表中单独保存；注释定位沿用相同版本化定位结构，支持范围起止。

永久位置保存内容锚点，不保存排版页码。字号、字体、窗口变化时重新分页；文本摘要或内容版本改变时不机械沿用旧偏移，可根据引用上下文恢复，恢复失败回退章节开头并提示用户。PDF 等其他格式后续增加不同 `kind`，不强行套用文字偏移。

`progress_ratio` 范围为 0～1，允许 NULL；`progress_source` 标注估算或远端值。章节平均比例只能标记 `estimated`。多设备冲突先比较不透明远端版本或已确认的服务端时间语义；未同步本地位置与变化后的远端位置同时保留，让业务层选择。不能只取较大百分比，因为用户可以往回读。

### 4.5 笔记、书签和划线

`annotations` 用一个表承载三种数据，公共字段、定位和同步生命周期一致：

| `kind` | 数据 |
| --- | --- |
| `note` | `note_text` 保存想法/笔记，`quote_text` 可存引用原文 |
| `bookmark` | 保存一个定位点，可用 `note_text` 保存用户标题 |
| `highlight` | `locator_json` 保存范围，`quote_text` 保存划线原文，`style_json` 保存样式 |

`annotation_id` 为本地随机标识，创建离线笔记时即可确定；`remote_id` 是服务端返回的身份，尚未上传时为 NULL。远端身份按账号、书籍、类型唯一，不能直接使用本地随机标识 冒充远端 ID。同一处划线和相关想法可以是不同记录，后续确有协议关系时再增加关联字段。

本地修改递增 `revision`，同时写 outbox。删除先设置 `deleted_at_ms` 墓碑并排队，云端确认后再物理清理；仅本地且从未尝试发送的记录可直接删除。ACK 只将对应 revision 标记同步成功，不能覆盖发送期间产生的新修改。第一阶段可以只读云端笔记并支持本地书签；远端书签语义、划线原文映射和写接口未验证前，不发送这些写任务。

### 4.6 网络同步与阅读时长

| 表 | 用途 |
| --- | --- |
| `sync_cursors` | 保存账号级书架、每本书目录/笔记/进度的同步游标、最近成功/尝试时间与错误类别 |
| `sync_jobs` | 持久化待上传的进度或注释快照、实体版本、状态、重试次数和下次尝试时间 |
| `reading_intervals` | 按书记录实际前台阅读时长，绑定某个同步任务，确认后标记 `acknowledged_at_ms` |

游标与对应数据在同一事务提交；请求失败不推进游标。每本书的同步资源删除时，显式清理其 `sync_cursors` 和 `reader_settings`；这两张表使用作用域键，不与 `books` 建外键。

同步任务状态：

```text
pending / retry → in_flight → done
                          → retry       已确认可安全重试
                          → uncertain   请求可能已生效，但结果未知
                          → blocked     权限、验证要求、协议不支持或冲突
```

发出请求前先提交 `in_flight` 和 `attempt_auth_epoch`。重启后遗留的 `in_flight` 转为 `uncertain`，不能一律重发。业务明确拒绝、尚未发送和结果不确定分别处理；仅确认可重试时使用退避。`job_id` 只能保证本机去重，除非服务端协议明确支持该幂等键，否则不能宣称服务端恰好执行一次。

`payload_json` 是带 schema_version 的业务快照，不保存 token、临时签名或完整 HTTP 请求；发送时根据当前有效会话构造请求。任务创建后快照不可修改，且同一本书的写任务串行发送。尚未发送且不含时长的旧位置任务可以合并到最新 revision；已发送/不确定的任务不能直接替换或吞掉。

阅读时长使用单调时钟累计实际前台活动间隔，`active_ms` 独立保存；墙钟仅用于区间记录，时钟跳变时切分区间并规范化结束时间。睡眠、后台或长时间无活动不计入。每个区间只能绑定一个对应账号、书籍、`kind=progress` 的任务，这项跨表业务约束由事务内的 Rust 检查保证。

上传确认时，在一个事务中将任务设为 `done`，将其绑定区间设为已确认，并仅确认该实体版本。结果不确定的时长不能自动再次计入下一任务。`prune_completed_jobs` 先删除已确认区间再清理已完成任务；外键禁止直接删除仍有关联区间的任务。正文/进度写接口当前仍待验证，设计就绪不代表可以直接启用上传。

### 4.7 阅读设置

`reader_settings(account_id, book_id)` 的空 book_id 保存账号默认设置，非空保存单书覆盖；JSON 包含 `schema_version`、字体名、字号、行距、页边距、主题等。最终设置按“内置默认 → 账号默认 → 单书覆盖”合并。设备旋转、屏幕刷新等设备配置继续单独管理，不与云端阅读数据混在一起。

## 5. Rust 模块边界

建议新增独立 `src/storage/`，让协议层和 UI 都不直接执行 SQL：

```text
src/storage/
├── mod.rs           # Storage::open、连接配置、事务入口
├── models.rs        # Identity、Locator、Annotation、SyncJob 等
├── migrations.rs    # include_str! 建表 SQL，user_version 顺序升级
├── session.rs       # 会话保存、轮换、登出、auth_epoch 检查
├── library.rs       # 书籍、书架、目录的事务更新与查询
├── reader.rs        # 本地/远端位置、注释、设置和阅读区间
├── files.rs         # 文件提交、索引校验、恢复与 LRU
└── sync.rs          # 游标、outbox 领取、ACK、失败与冲突状态
```

使用 `rusqlite 0.41.0` 的 bundled SQLite 构建方式，随应用静态编译 SQLite，避免依赖 Kindle 系统 sqlite3 动态库；依赖固定在 Cargo.lock。没有引入 ORM 或数据库连接池。ARM C 头文件通过 `scripts/setup-musl-headers.sh` 准备，源码与许可随发布源码包保留。

数据库连接由单一存储工作线程持有，命令通过 channel 串行执行，通过 oneshot 返回结果。现有 `src/app.rs` 网络 worker 可 await 存储响应；SQLite 查询与文件 fsync 不在 Slint UI 线程执行。网络 await 不放在数据库事务中。文件下载可并发，文件索引提交统一经过存储线程。

建议公开业务接口（签名示意，不是已实现 API）：

```text
load_session() -> Option<Session>
save_session(credentials, expected_identity)
logout(expected_identity)
load_shelf(account_id) -> Vec<Book>
apply_shelf_snapshot(identity, books, cursor)
apply_chapter_delta(identity, book_id, delta, cursor)
commit_cached_file(identity, metadata, bytes) -> FileId
read_cached_file(identity, resource) -> Option<Vec<u8>>
save_position_and_enqueue(identity, position, intervals, enqueue)
save_annotation_and_enqueue(identity, annotation, enqueue)
claim_sync_job(identity, now_ms, enabled_kinds) -> Option<SyncJob>
complete_sync_job(identity, job_id, expected_revision, remote_id, remote_revision)
record_sync_failure(identity, job_id, failure_class)
```

账号 ID 只负责数据归属；能改变会话、网络同步结果或文件索引的异步写操作同时验证 epoch。首次恢复先显示本地书架，再尝试网络更新，网络失败仍保留本地数据。

## 6. 文件与数据库一致性

文件系统和 SQLite 不能组成同一个原子事务，使用以下提交顺序：

1. 下载到同一数据根目录的 `.part`，完成格式、身份、完整性和大小校验。
2. flush/fsync 文件，rename 到新的唯一正式路径，再同步父目录；不覆盖仍被旧索引引用的文件。
3. 数据库事务插入新索引并删除同资源旧索引；提交前再次验证账号 epoch。
4. 提交成功后删除不再引用的旧文件。提交失败留下的孤立文件由恢复扫描清理。

缓存淘汰先事务删除索引，再移除物理文件。启动或空闲时清理超时 `.part`、缺失文件索引和未被引用的文件；扫描必须避开当前正在写入的文件。磁盘满时不更新索引，也不删除尚未成功替换的旧文件。清缓存只处理 `cached_files` 与文件，不清阅读位置、笔记和同步任务。

## 7. SQLite 配置、迁移与接入次序

当前模型只有一个存储工作线程，初始采用 `journal_mode=DELETE`、`synchronous=FULL`、`busy_timeout=5000`，并在每个连接、事务外开启 `foreign_keys=ON`。如后续确需独立读连接，再评估 WAL；它需要处理 `-wal`、`-shm` 和 checkpoint，不是当前单连接结构的前提。[SQLite 外键说明](https://www.sqlite.org/foreignkeys.html)、[WAL 说明](https://www.sqlite.org/wal.html)、[PRAGMA 配置](https://www.sqlite.org/pragma.html)。

使用 `PRAGMA user_version` 管理 schema：新库为 0，成功执行 `001_initial.sql` 后为 1，再通过 `002_annotation_conflicts.sql` 升级到版本 2，再通过 `003_shelf_groups.sql` 升级到当前版本 3，增加账号隔离的书架分组。v2 为注释增加 `remote_snapshot_json`，保存最近云端快照，冲突时保留本地与云端两个版本。迁移按版本依次执行，失败回滚，遇到更高版本拒绝写入而不是重建。初始 SQL 自带事务，不可再套一层事务，也不能每次启动重复执行；后续升级不要重放此文件或覆盖已有版本。迁移前通过 SQLite backup API 或停写并关闭数据库后复制做备份，不直接复制正在写入的数据库。

实施顺序：

1. 引入 SQLite、连接配置、迁移和存储线程。先接会话、书籍、书架和封面索引，保留原有 UI 行为。
2. 仅在从 v0 创建数据库时，校验并导入有效旧 `session.json`，会话和 schema 在同一事务中提交，迁移完成后删除旧文件。损坏或字段不完整的旧会话不导入，用户需重新登录。若提交后删除失败，下次根据已有 schema 跳过导入并继续清理；登出后不得重新导入旧会话。旧封面可清理或按需重建，不必批量迁移。
3. 接目录和正文文件索引、阅读位置与设置，验证重排版后的定位恢复。
4. 接笔记、书签、划线的本地保存和云端读取；经真实协议验证后启用对应 outbox 上传。
5. 接进度冲突处理和阅读时长确认；补充断网重启、结果不确定、账号切换及磁盘满场景验证。

SQL 验证应包含：完整建库、外键和约束、账号隔离、移出书架保留用户数据、事务失败回滚、区间绑定与任务清理、删除账号后的级联。实际 Rust 接入后另验证旧会话迁移、文件提交中断、token 轮换及静态 ARM 构建。设计与本地 SQL 检查不能替代设备断电测试或云端协议联调。


## 8. 当前 API 使用

`Store::new(path)` 创建线程，返回 `Result<Store>`；调用 `ready().await` 可提前检查初始化错误。所有存储方法都是异步请求，克隆 `Store` 共享同一线程；最后一个句柄释放时排空队列并关闭线程。`.storage.lock` 用进程级文件锁防止两个存储实例同时执行文件恢复，文件保留不代表锁仍被占用。

```rust,no_run
use rereader::storage::Store;

# async fn example() -> rereader::weread::Result<()> {
let store = Store::new(Store::default_path()?)?;
store.ready().await?;
if let Some(identity) = store.identity().await? {
    let books = store.load_shelf(&identity).await?;
    let settings = store.load_settings(&identity, None).await?;
    // 网络响应提交使用同一个 identity，不能重新取得最新身份冒充旧请求来源。
}
# Ok(())
# }
```

位置与注释保存接口中的 `revision` 为预期旧版本，新对象传 0，返回对象包含递增后的新版本；旧版本写回会报错。`save_position_and_enqueue`、`save_annotation_and_enqueue` 的 `enqueue` 参数显式控制是否创建网络任务，未验证协议时传 false。阅读区间 ID 必须稳定且唯一，重复插入整体回滚；后续启用同步时会把该书未绑定的区间一起加入任务。

同步使用 `claim_sync_job(identity, now_ms, enabled_kinds)` 领取已验证类型。同一本书按任务插入顺序串行发送。注释首次创建成功后可能才获得 remote_id；后续任务发送前调用 `prepare_sync_payload`，它结合当前 ID 映射生成业务载荷，保留旧任务快照不变。ACK 需包含任务的精确 revision；注释写入 ACK 必须提供服务端身份。`reconcile_sync_job` 只能在查询云端证明已执行或未执行后调用，不能把超时推测为未执行。

云端注释同步通过 `apply_remote_annotations` 按远端 ID 合并；本地脏数据遇到云端变化时保留冲突，`load_annotation_conflict` 可读取云端候选。`resolve_annotation_conflict` 显式选择本地或云端版本，存在未确认请求时拒绝解决冲突。单书删除与账号删除会清理关联数据及物理文件；移出书架与 LRU 不删除用户笔记和位置。

文件接口目前接收完整且已由协议层验证的 `Vec<u8>`，单个资源限制 256 MiB；适合章节和图片，超大整书的流式提交需要后续扩展。读取时校验大小和 SHA-256，返回字节而不向 UI 暴露可任意拼接的路径。封面存储失败仍显示当次下载结果；旧 `covers/` 缓存不导入，可在应用退出后手动清除，后续按需重建索引。
