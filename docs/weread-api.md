# rereader 微信读书墨水屏客户端接口

调研日期：2026-10-26。本文面向 Rust + Slint 的 Kindle/桌面原生阅读器，统一采用墨水屏客户端协议。

基地址为 `https://i.weread.qq.com`，业务请求通过 `vid`、`accessToken` 请求头鉴权，使用 `refreshToken` 刷新会话。本文列出接口速查；设备配置、登录签名、传输行为和 Rust 移植安排见 [weread-omni 源码研究](weread-eink-research.md)。

当前参考 SDK 已实现登录、书架、目录和进度读取；普通电子书正文下载与进度写回仍只有端点记录，需要进一步验证。本文完成源码核对，尚未进行账号联调或 Kindle 真机验证。

## 1. 核心接口清单

本节以 `weread-omni` 源码为主。除特别注明外，以下接口存在 SDK 调用实现；完整客户端端点目录还包含许多仅从客户端整理出来的路径，不能视为全部已实现或已联调。

### 1.1 登录与会话

| 步骤 | 方法及地址 | 主要字段/结果 |
| --- | --- | --- |
| 获取微信登录票据 | `GET https://i.weread.qq.com/wxticket?nonceStr=weread` | `signature`、`timeStamp` |
| 创建二维码登录会话 | `GET https://open.weixin.qq.com/connect/sdk/qrconnect` | `appid`、`noncestr`、`timestamp`、`scope`、`signature`；返回 `uuid` |
| 二维码承载链接 | `https://open.weixin.qq.com/connect/confirm?uuid=...` | 在本机绘制二维码供用户扫码 |
| 轮询确认 | `GET https://long.open.weixin.qq.com/connect/l/qrconnect` | `f=json`、`uuid`、`last`；确认后返回 `wx_code` |
| 换取客户端凭据 | `POST https://i.weread.qq.com/login` | `code`、设备/安装信息、时间戳、随机值和签名；返回 `vid`、`accessToken`、`refreshToken` |
| 刷新客户端凭据 | `POST https://i.weread.qq.com/login` | 改用 `refreshToken`，并携带设备信息和签名 |

参考实现区分的二维码状态：`404` 已扫描、`405` 已确认、`402` 过期、`403` 拒绝、`408` 等待超时。这些是该微信轮询协议的业务状态，不是 HTTP 状态码。

业务请求携带 `vid`、`accessToken`，还带客户端版本和平台头：`User-Agent`、`appver`、`basever`、`baseapi`、`osver`、`channelId`。参考实现使用墨水屏 2.1.2 的设备配置，这不是 Kindle 原生客户端的正式接入配置。

设备身份应持久化；刷新请求合并为一次执行，并保存服务器轮换后的凭据。刷新失败后应回到登录界面，不能无限循环请求。

来源：[二维码登录][native-login]、[令牌刷新][native-token]、[身份配置][native-profile]、[版本头][native-device]。

### 1.2 书架、目录和搜索

下表基地址为 `https://i.weread.qq.com`。GET 字段放查询参数，POST 使用 JSON。

| 功能 | 方法/路径 | 源码中的请求字段 |
| --- | --- | --- |
| 书架 | `GET /shelf/sync` | SDK 当前不传业务参数；客户端目录另列同步游标 |
| 加入书架 | `POST /shelf/add` | `bookIds: [id]`、`albumIds: []`、`archiveIds: []` |
| 移出书架 | `POST /shelf/delete` | 同上 |
| 搜索 | `GET /store/search` | `keyword`、`scope`、`count`、`maxIdx` |
| 搜索建议 | `GET /store/suggest` | `keyword`、`count` |
| 书籍信息 | `GET /book/info` | `bookId` |
| 目录 | `POST /book/chapterInfos` | `bookIds: [id]`、`synckeys: [0]`、`updateTimes: [0]`、`maxfreeIdx: [0]` |
| 读取进度 | `GET /book/getProgress` | `bookId` |
| 标记读完 | `POST /book/markstatus` | `bookId`、`status: 4`、`isCancel`、`auto: 0`、`finishInfo: 0` |

目录原始响应的章节位于 `data[].updated`；SDK 将其转换为 `chapters`。不要把 SDK 返回类型当作服务端原始 JSON。

路径区分大小写，目录与进度读取分别使用 `/book/chapterInfos`、`/book/getProgress`。

`markstatus` 是阅读状态变更，不能代替章节偏移和阅读时长上报。

来源：[书籍实现][native-book]、[书架实现][native-shelf]、[搜索实现][native-search]。

### 1.3 划线与想法

| 功能 | 方法/路径 | 关键字段 |
| --- | --- | --- |
| 个人划线列表 | `GET /book/bookmarklist` | `bookId`、`synckey` |
| 新增划线 | `POST /book/addBookmark` | `bookId`、`chapterUid`、`range`、`markText`、`type`、`style`；可选 `bookVersion` |
| 更新划线样式 | `POST /book/updateBookmark` | `bookmarkId`、`style`，可选 `colorStyle` |
| 删除划线 | `POST /book/removeBookmark` | SDK 实际使用 `bookmarkIds: [id]` |
| 个人想法列表 | `GET /review/list` | `bookId`、`mine: 1`、`listType: 1`、`listMode: 0`、`synckey`、`count` |

`range` 必须对应平台的原文定位规则。客户端重新排版后的页码或 Slint 字符串下标，不能直接作为划线范围。第一版可先读取划线，等正文偏移映射验证后再开放写入。

来源：[笔记实现][native-notes]。其他书评写入接口可继续查阅[端点目录][native-catalog]，不纳入阅读 MVP。

## 2. 正文下载与进度写回：待验证接口

客户端整理文档列出：

| 功能 | 方法/路径 | 已整理的字段示例 | 当前缺口 |
| --- | --- | --- | --- |
| 章节下载 | `GET /book/chapterdownload` | `bookId`、`bookType`、`bookVersion`、`chapters`、`offline`、`pf`、`pfkey` 等 | 下载凭据、响应格式、资源解析和权限语义待确认 |
| 书籍下载 | `GET /book/download` | `bookId` | 不能据此认定返回完整可读 EPUB |
| 阅读上报 | `POST /book/read` | `bookId`、`chapterUid`、`chapterOffset`、`readingTime`、时间戳及签名等 | 签名、必填组合、偏移单位和时长口径待确认 |
| 批量进度 | `POST /book/batchUploadProgress` | `books`、`timestamp`、`random`、`signature` 等 | 队列重放、去重和每本书的数据结构待确认 |

这些接口没有出现在参考 SDK 的高层正文/进度写入实现中。这里只记录调查入口，不提供未经验证的“可直接运行”请求。

来源：[墨水屏客户端端点目录][native-catalog]。

## 3. rereader 的接入安排（建议，尚未实现）

1. **先验证阅读可行性**：用自己的账号和有权阅读的一本书，确认登录、目录、正文展示、进度写回及官方客户端读回。如果只有书架和目录可用，应保留“正文暂不可用”的状态。
2. **数据浏览原型**：使用已选定的墨水屏客户端会话做书架、书籍详情和目录 UI，具体调用顺序见专门研究文档。
3. **阅读 MVP**：补齐墨水屏客户端会话与正文协议，接入登录、书架、目录、单章正文和本地分页。优先验证纯文本章节，再处理 HTML、图片和复杂 EPUB。
4. **进度同步**：本地保存书籍、章节身份、原文偏移和内容版本；网络失败保存待同步记录；启动或切书时读取云端进度并处理冲突。只统计真实前台阅读时间。
5. **笔记与搜索**：正文偏移可靠后增加划线写入；书城搜索、加入书架、热门划线放在后续阶段。

建议将网络接口放在独立 `weread` 模块，向 UI 暴露业务操作，而不是直接暴露 HTTP 路径：

```text
Session: 登录 / 刷新 / 登出
Shelf:   列表 / 详情 / 目录
Content: 单章内容 / 资源加载
Reader:  本地位置 / 云端位置 / 同步
Notes:   读取 / 创建 / 删除
```

网络、内容解析和磁盘操作放在工作线程；通过 Slint 的事件循环更新 UI。桌面和 Kindle 共享数据层，平台差异继续由显示/输入后端处理。

## 4. 联调必须确认的契约

| 问题 | 验证方式 |
| --- | --- |
| 会话是否真正可用 | 区分 HTTP 状态和业务 `errcode`/`errCode`，不能仅判断 HTTP 200 |
| 凭据过期 | 参考客户端将 `-2012` 视为会话过期；刷新一次后失败则重新登录，错误含义仍须按接口确认 |
| 限流或人机验证 | 参考 SDK 将 `-2014` 视为限流、`-2041` 视为人机验证；记录分类，不循环撞接口 |
| 目录与正文对应关系 | 选含前言、卷标题、缺号章节的书，确认 UID 和顺序分别处理 |
| 偏移单位 | 用中文、换行、emoji 对照云端位置，确认字节、Unicode 码点、UTF-16 或其他原文定位规则 |
| 会员/购买/试读权限 | 使用账号当前有权访问的内容验证；将试读终点、无权限、下架与网络错误分别呈现 |
| 同步可靠性 | 测试断网、重启、多设备更新；非幂等写入发生不确定失败时先查询状态，避免盲目重发 |
| 本地缓存 | 按账号隔离；登出清除凭据；缓存不等于拥有长期离线阅读权限 |

不要将 `accessToken`、`refreshToken` 等登录凭据写入日志、仓库和可公开的请求样例。现阶段没有进行账号联调，也未确认微信读书对第三方原生客户端的完整接入与分发安排。

错误处理参考：[客户端传输层][native-transport]。

## 5. 固定来源快照

参考项目为 [teng-lin/weread-omni](https://github.com/teng-lin/weread-omni)，研究提交固定为 `88bd2e095d7d7ee423eaadf8f40653e72c5be6d4`。下方源码链接均指向该快照。

第三方实现可作协议证据，不能代替服务端的兼容性承诺。后续接入前应刷新来源并进行账号实测。

[native-login]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/qrlogin.ts
[native-token]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/token.ts
[native-profile]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/profile.ts
[native-device]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/device-ua.ts
[native-book]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/book.ts
[native-shelf]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/shelf.ts
[native-search]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/search.ts
[native-notes]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/notes.ts
[native-catalog]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/docs/endpoints.md
[native-transport]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/mobile.ts
