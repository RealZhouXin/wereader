# wxBook 墨水屏阅读接口研究

研究日期：2026-10-26。参考 [danleegle/wxBook](https://github.com/danleegle/wxBook)，已查询远端 HEAD，固定快照为 `564df2111135c6a7011d24d2aaa9edabad992e92`。

本文核对源码，未登录用户账号、抓取真实阅读流量、运行 ESP32 固件或验证 Kindle。本文记录的是调用实现和源码风险，不将其视为服务端稳定契约。

## 1. 最重要的发现

`wxBook` 是运行在 ESP32 墨水屏上的阅读应用，但 HTTP 层使用 `https://weread.qq.com` 的 Web 协议：Cookie 登录、阅读器页面会话参数、正文分片、阅读上报。它没有调用 Android 墨水屏客户端的 `/book/chapterdownload`。

| 项目 | weread-omni | wxBook |
| --- | --- | --- |
| 服务主机 | `i.weread.qq.com` | `weread.qq.com` |
| 会话 | `vid` / `accessToken` 请求头 | `wr_vid` / `wr_skey` / `wr_gid` / `wr_rt` Cookie |
| 登录 | 微信 SDK 二维码 → `/login` | `/api/auth/getLoginUid`、`getLoginInfo` |
| 正文 | 下载端点记录，缺实现 | 阅读器页面 → `/web/book/chapter/...` 分片 |
| 进度 | 已实现读取，写回只有端点记录 | `/web/book/read` 有调用实现 |

因此，该项目补充了“墨水屏设备如何完成阅读”的代码证据，但没有补齐当前选定的客户端协议。不能将 Cookie、分片字段和签名混入 `weread-omni` 会话。此处单独保留研究材料，主接入路线仍见 [墨水屏客户端研究](weread-eink-research.md)。

源码入口：[weread_client.cc][client]、[weread_crypto.cc][crypto]、[weread_app.cc][app]。上游还有[协议说明][upstream-doc]。

## 2. 实际接口清单

以下路径全部基于 `https://weread.qq.com`。除书架和登录等 GET 外，请求体采用 JSON。

| 功能 | 方法/路径 | 输入与响应处理 |
| --- | --- | --- |
| 创建扫码会话 | `GET /api/auth/getLoginUid` | 读取顶层或 `data.uid` |
| 二维码地址 | `/web/confirm?uid=...` | 在设备上编码为二维码 |
| 轮询登录 | `GET /api/auth/getLoginInfo?uid=...&otp` | 需要验证码时传 `otp=四位码`；吸收响应 Cookie |
| 登录续期 | `POST /web/login/renewal` | `rq: "%2Fweb%2Fbook%2Fread"`、`ql: false`；要求 `succ=1` |
| 书架与进度 | `GET /web/shelf/sync` | 将 `books` 与 `bookProgress` 按 `bookId` 合并 |
| 目录 | `POST /web/book/chapterInfos` | `bookIds: [id]`；读取 `data[].updated`，兼容 `chapterInfos` |
| 阅读器页面 | `GET /web/reader/{EncodeId(bookId)}k{EncodeId(chapterUid)}` | 从 HTML 提取字符串 `psvts` |
| EPUB 分片 | `POST /web/book/chapter/e_0`、`/web/book/chapter/e_1`、`/web/book/chapter/e_3` | 组合正文分片后解码、提取文字 |
| TXT 分片 | `POST /web/book/chapter/t_0`、`/web/book/chapter/t_1` | 在前置格式判断后读取 |
| 阅读上报 | `POST /web/book/read` | 位置、估算百分比、阅读秒数及会话/签名字段；要求 `succ=1` |

证据：[客户端实现][client]。源码设置浏览器 UA、`Origin`、`Referer`、Cookie 和 JSON Content-Type；仅添加 `/web` 前缀不足以复用客户端请求。

## 3. 登录与续期

[WereadAuth][auth] 保存 `vid`、`skey`、`gid`、`refresh_token`。本地登录判定为 `vid` 非空且 `skey` 或 `gid` 至少一项非空，这不是服务器认可会话有效的证明。

`PollQrLogin` 处理：

- `succeed=true`：使用响应 Cookie；字段缺失时尝试从 `webLoginVid` / `vid` 和 `accessToken` 补充本地身份。
- `NEED_OTP`、`OTP_NOT_MATCH`：展示验证码输入或错误。
- `LOGIN_TIMEOUT`、`OTP_EXPIRED`：视为过期。
- 其他非空 `logicCode`：返回登录错误。

续期走 `/web/login/renewal`，成功后应用 `Set-Cookie`；凭据变化由业务层保存到 NVS。[存储实现][auth-store] 使用 ESP-IDF NVS，移植到 Rust 时需要改成账号隔离的持久化存储。

`wr_rt` 是这套 Cookie 会话里的字段，不能直接作为墨水屏客户端 `/login` 的 refreshToken 使用。

## 4. 正文请求链路

```text
bookId + chapterUid
  → ReaderTokens
  → 编码阅读器 URL
  → 流式读取 HTML，提取 psvts
  → 请求 e_0，判断内容分支
  → TXT: t_0 + t_1
     EPUB 分片: e_0 + e_1 + e_3
  → 分片校验与解码
  → TXT 保留文本 / EPUB 提取文本
  → 本地缓存、分页
```

`FetchReaderToken` 分块读取 HTML，找到 `psvts` 后提前结束，避免将整页加载进内存；扫描缓冲超过 8192 字节时保留尾部 256 字节，继续搜索。`ReaderTokens` 在内存缓存最近 8 本书的令牌。

分片请求字段来自 [ContentParams][crypto]：

| 字段 | 参考实现用途 | JSON 类型 |
| --- | --- | --- |
| `b`、`c` | 编码后的书籍/章节身份 | string |
| `ct` | 秒时间戳 | number |
| `r` | 随机值平方 | number |
| `ps`、`pc` | 页面会话值与时间编码值 | string |
| `sc`、`st` | 参考实现固定为 1、0 | number |
| `prevChapter` | 参考实现传字符串 `"false"` | string |
| `s` | 参数排序、编码后计算的校验值 | string |

参数先以字符串形式参与签名，再由 `ParamsJson` 将 `ct/sc/st/r` 写成 JSON 数值。源码特别指出：将这些数值全部写为字符串可能得到空对象响应。Rust 移植应分别管理签名表示和 JSON 类型。

`e_0` 失败或返回空对象时，源码强制刷新页面令牌并重试一次；分片调用之间插入约 400ms 间隔。这个间隔是客户端策略，不能当作服务端公布的限速。

格式判断与限制：

- `e_0` 以 ZIP 标记 `PK` 开头时，明确返回“暂不支持整包 EPUB”。
- 空响应或带 `bookId` 的 JSON 被用于选择 TXT 分支；这是源码启发式判断，错误响应也要独立验证。
- 常规 EPUB 分片解码后经 `StripMarkup` 转成文字，未实现完整 EPUB 排版及图片显示。

`DecodeShards` 的主要处理为分片 MD5 检查、拼接、位置还原和 Base64 解码。函数存在不等于所有书籍格式都可处理；当前快照也不是通用电子书解密库。

## 5. 进度上报与本地位置

`UploadProgress` 先获取阅读会话参数，再发送 `/web/book/read`。字段来源如下：

| 字段 | 参考实现含义 |
| --- | --- |
| `appId` | 从固定 UA 构造的 Web 客户端标识 |
| `b`、`c` | 编码后的书籍/章节身份 |
| `ci`、`co` | 章节序号、非负章节内偏移 |
| `pr` | 限制到 0～100 的进度值 |
| `rt` | 限制到 0～3600 的阅读秒数 |
| `ct`、`ts`、`rn` | 秒时间戳、毫秒量级值、随机值 |
| `ps`、`pc`、`sg`、`s`、`sm` | 页面会话、编码及校验相关字段 |

具体构造见 [UploadProgress][client]，不能以客户端 `/book/read` 的长字段名或登录签名代替。HTTP 成功且 `succ=1` 只代表源码判定调用成功；跨设备位置和统计是否正确仍需官方客户端读回。

[BuildPages][app] 按 UTF-8 解码出的码点计数，记录每页开头的章节文字偏移；重新分页后选出不超过目标偏移的最后一个页面。这个模型可以移植，但它统计的是处理后的文本，不能直接证明等同于服务器原文偏移或 UTF-16 下标。

`CurrentBookPercentLocked` 按“章节下标 + 当前章节页比例”除以总章节数估算百分比，未按章节字数加权。rereader 不应将该值称为平台精确阅读百分比。

业务层每到页下标为 5 的倍数时尝试排队，退出阅读或切章等时机也排队；待上报记录保存到 NVS。阅读时间依据前台阅读页活动间隔累计，单次间隔最多计入 300 秒，单次排队秒数最多 3600。这些均为固件策略，未证明服务端采用同样统计口径。

## 6. 可借鉴设计与移植前需修正的风险

以下风险来自源码检查，未通过实际故障实验验证。

| 源码行为 | 对 rereader 的影响与安排 |
| --- | --- |
| `Request` 在连接层失败后无条件重试，包括 POST | 上报可能已成功但响应丢失，重发可能重复计时；需要区分幂等读取和结果不确定的写入 |
| `RunProgressLoop` 上报失败后续期再上报 | 不能将所有失败都归为会话过期，应保留错误分类和重复提交语义 |
| `DecodeShards` 跳过校验失败的分片，而非立即整体失败 | 应校验必需分片完整性，避免部分内容被当成完整章节缓存 |
| `ReaderTokens` 全局缓存只以 `bookId` 为键 | 应加入账号/会话代次，并在登出和重新登录时清空 |
| 章节缓存文件名和头部验证书籍/章节身份 | 这是内容身份校验；当前键和头部未包含账号或内容版本，移植应补充 |
| `QueueProgressSyncLocked` 用单个待上报槽携带旧秒数 | 跨书合并时可能把旧书时间归到新书；应按账号和书籍分别排队 |
| 进度任务回写凭据主要检查 `vid` | 同账号重新登录仍可能接收旧结果；需要会话代次屏障 |
| `StripMarkup` 先转换成纯文本再分页 | 原文偏移映射、图片和复杂段落需独立设计 |

可以直接借鉴的设计思路包括：页面令牌流式提取、章节缓存临时文件加重命名、书籍/章节头部校验、相邻章节预取、前台优先，以及待同步数据持久化。ESP32 的 NVS、SPIFFS、FreeRTOS 和显示驱动不适用于当前 Rust/Slint 项目，需要替换实现。

## 7. 对当前墨水屏客户端路线的结论

本次新增了正文分片和进度上报的实际代码参考，但不能据此宣称已掌握 `i.weread.qq.com/book/chapterdownload` 的响应格式、下载凭据或 `/book/read` 的签名。

继续采用当前路线时，下一步仍需研究墨水屏 APK 的 `BookDownloadService` 和 `ReportService`。`wxBook` 可以指导我们识别会话依赖、分片完整性、正文位置和同步队列问题；其具体网络协议应保持独立。

若后续明确采用这套协议，应完整实现其 Cookie 登录、续期、页面令牌、正文和同步适配器，而不是只把分片接口接到原有 token 会话。本文没有更改主路线或应用运行代码。

验证优先级：本人账号可读书籍的单章正文 → 中文/换行/emoji 偏移 → 手机上读回位置 → 真实时长统计 → 断网、重启与切换账号。没有找到该仓库专门覆盖阅读协议的自动化测试，本次未运行固件构建。

[client]: https://github.com/danleegle/wxBook/blob/564df2111135c6a7011d24d2aaa9edabad992e92/main/weread/weread_client.cc
[crypto]: https://github.com/danleegle/wxBook/blob/564df2111135c6a7011d24d2aaa9edabad992e92/main/weread/weread_crypto.cc
[app]: https://github.com/danleegle/wxBook/blob/564df2111135c6a7011d24d2aaa9edabad992e92/main/weread/weread_app.cc
[auth]: https://github.com/danleegle/wxBook/blob/564df2111135c6a7011d24d2aaa9edabad992e92/main/weread/weread_auth_store.h
[auth-store]: https://github.com/danleegle/wxBook/blob/564df2111135c6a7011d24d2aaa9edabad992e92/main/weread/weread_auth_store.cc
[upstream-doc]: https://github.com/danleegle/wxBook/blob/564df2111135c6a7011d24d2aaa9edabad992e92/docs/weread.md
