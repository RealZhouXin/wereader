# 微信读书墨水屏 API 协议

更新日期：2026-10-27。适用于 rereader 的 Rust + Slint Kindle/桌面客户端。

本文合并原接口清单、weread-omni 研究、weread-kindle 研究、下载/解密实测及旧解包算法分析，作为墨水屏客户端协议的统一维护入口。协议主机为 `https://i.weread.qq.com`，业务请求通过 `vid`、`accessToken` 请求头鉴权。

## 1. 协议范围与验证状态

本文记录第三方固定源码快照和本项目已有实测，不能作为服务端兼容性承诺。合并文档期间没有发送账号请求、刷新凭据或写入云端数据。

| 能力 | 已有证据 | rereader 当前状态 |
| --- | --- | --- |
| 扫码登录、刷新 | weread-omni / weread-kindle 均有实现 | 已接入登录、会话保存/恢复 |
| 书架 | 两个参考项目均有实现 | 已接入电子书书架、分页、封面缓存 |
| 用户阅读时长 | 已有端点依据和本地响应测试 | 已接入累计/本周时长，真实字段与周边界待账号联调 |
| 元信息、目录、EPUB 下载 | 已有指定账号单章请求记录 | 研究脚本已验证；目录/正文阅读 UI 尚未接入 |
| EPUB 解包 | 已有密码、CRC、章节身份、XHTML 校验记录 | 仅一本书一章样本，尚未接入 Rust 正文模块 |
| TXT、图片、其他权限场景 | weread-kindle 有处理实现 | 尚未完成本项目实测 |
| 阅读位置/时长写回 | weread-kindle 有字段与签名实现 | 未完成本项目写回和官方客户端读回验证 |
| 划线、书签、想法 | 参考项目有读写实现 | 存储层已有支持，远端协议与阅读 UI 尚未接入 |
| 整书下载、批量进度 | weread-omni 只有端点记录 | 请求结构、响应、签名及去重语义待确认 |

`wxBook` 使用 Web Cookie 和 `/web/...` 协议，不合并进本文的 App 鉴权、正文或签名算法；其独立分析保留在 [wxBook 研究](wxbook-interface-research.md)。

目录：

- [设备配置与通用契约](#2-设备配置与通用契约)
- [扫码登录、刷新与会话](#3-扫码登录刷新与会话)
- [接口清单及参数差异](#4-接口清单及参数差异)
- [正文下载与解包](#5-正文下载与解包)
- [阅读进度与时长签名](#6-阅读进度与时长签名)
- [笔记定位与原文偏移](#7-笔记定位与原文偏移)
- [传输、错误与重试](#8-传输错误与重试)
- [已有实测与复测方式](#9-已有实测与复测方式)
- [旧算法对照](#10-旧算法对照)
- [本项目接入与验证](#11-本项目接入与验证)
- [固定来源](#12-固定来源)

## 2. 设备配置与通用契约

GET 参数放查询字符串；POST 参数使用 JSON。HTTP 路径和 JSON 字段区分大小写。`vid` 在本地规范为字符串，版本号、章节 UID、目录顺序、同步游标和本地页码分别保存。

两个参考实现均模拟墨水屏 2.1.2，但采用不同设备配置：

| 字段 | weread-omni / rereader 现有配置 | weread-kindle |
| --- | --- | --- |
| `appver/basever` | `2.1.2.10245900` | 同左 |
| `channelId` | `900` | 同左 |
| `baseapi` / `osver` | `30` / `11` | `24` / `7.0` |
| `deviceName` / `deviceType` | `BOOX` / 数字 `3` | `Kindle` / 数字 `3` |
| 设备 ID | `eink334691225` + 19 位随机数字 | 16 随机字节的十六进制文本 |
| 安装 ID | `eink31` + 26 位随机数字 | 阅读上报中使用设备 ID |
| UA | `WeRead/2.1.2 WRBrand/Onyx wr_eink Dalvik/2.1.0 (Linux; U; Android 11; BOOX Build/onyx)` | `WeRead/2.1.2 WRBrand/other Dalvik/2.1.0 (Linux; U; Android 7.0; Kindle)` |

这些是参考实现配置，不是正式的 Kindle 接入参数。设备身份应按会话持久化；字段组合集中管理，不因复制某个接口而切换整套请求身份。来源：[设备头][native-device]、[profile][native-profile]、[Lua 实现][direct]。

## 3. 扫码登录、刷新与会话

### 3.1 登录链路

| 步骤 | 方法及地址 | 主要字段/结果 |
| --- | --- | --- |
| 获取微信登录票据 | `GET https://i.weread.qq.com/wxticket?nonceStr=weread` | `signature`、`timeStamp` |
| 创建二维码登录会话 | `GET https://open.weixin.qq.com/connect/sdk/qrconnect` | `appid`、`noncestr`、`timestamp`、`scope`、`signature`；返回 `uuid` |
| 二维码承载链接 | `https://open.weixin.qq.com/connect/confirm?uuid=...` | 在本机绘制二维码供用户扫码 |
| 轮询确认 | `GET https://long.open.weixin.qq.com/connect/l/qrconnect` | `f=json`、`uuid`、`last`；确认后返回 `wx_code` |
| 换取客户端凭据 | `POST https://i.weread.qq.com/login` | `code`、设备/安装信息、时间戳、随机值和签名；返回 `vid`、`accessToken`、`refreshToken` |
| 刷新客户端凭据 | `POST https://i.weread.qq.com/login` | 改用 `refreshToken`，并携带设备信息和签名 |

参考实现区分的二维码状态：`404` 已扫描、`405` 已确认、`402` 过期、`403` 拒绝、`408` 等待超时。这些是该微信轮询协议的业务状态，不是 HTTP 状态码。

应用标识为 `wxab9b71ad2b90ff34`。scope 存在两套源码配置：weread-omni 使用 `snsapi_userinfo,snsapi_timeline,snsapi_friend`；weread-kindle 使用 `snsapi_userinfo,snsapi_friend,snsapi_favorites`。rereader 当前沿用前者，不把两组 scope 混成新的组合。

二维码可编码 `https://open.weixin.qq.com/connect/confirm?uuid=...`；weread-kindle 则保存扫码接口返回的 `qrcode.qrcodebase64` 图像。轮询的 `last` 是上次业务状态，不是 HTTP 状态码。weread-omni 的单次轮询超时 65 秒、总期限 5 分钟是客户端策略，不是服务端有效期保证。

### 3.2 登录防重放签名

```text
signature = lowercase_hex(SHA256(
    十进制毫秒时间戳 + deviceId + 十进制随机数
))
```

三项直接拼接，不加分隔符。扫码交换随机数为 0–999。

rereader / weread-omni 的扫码交换 JSON 包含 `appFirstInstall:1, code, deviceId, deviceName, installId, isAutoLogout:0, isFromQrcode:1, random, signature, timestamp, trackId:"", deviceType:3`。Lua 实现省略 `appFirstInstall/installId`，设备名为 `Kindle`；这是变体证据，不能推定字段普遍可省略。

成功时校验非空 `accessToken/refreshToken` 与合法 `vid`，不能只判断 HTTP 200。

### 3.3 刷新与账号隔离

刷新仍调用 `POST /login`，使用 `refreshToken`，携带设备信息、`inBackground:0, kickType:1, refCgi:"", trackId:""` 及同一套防重放字段。rereader 当前传 `deviceType:3`；Lua 另传 `wxToken:0`。weread-omni / rereader 刷新随机数为 1–1000，Lua 为 0–999。签名未拼入 refreshToken，不能将此规则推广到阅读上报。

- 同一会话的并发请求合并刷新，单个调用取消不应取消其他调用的等待。
- 刷新响应若带 VID，必须与当前账号一致；轮换后的凭据持久化。rereader 在刷新响应未带 refreshToken 时保留原值。
- 旧账号请求和刷新回调不能覆盖新登录状态，任务结果按账号/操作代次校验。
- 鉴权失败后的可重放请求最多重试一次；刷新失败进入重新登录，不无限循环。
- 凭据保存在权限受限的账号存储中，日志不输出 token、密钥或密码。

来源：[登录][native-login]、[刷新][native-token]、[凭据保存][credentials]、[Lua 登录][direct]。

## 4. 接口清单及参数差异

以下主表来自 weread-kindle 的 Lua 调用；其后补充 weread-omni 参数和本项目统计接口。没有单独标记实测的行均表示源码证据。

| 功能 | 方法/路径 | 关键请求与响应处理 |
| --- | --- | --- |
| 书架 | `GET /shelf/sync` | `synckey=0, lectureSynckey=0`；合并 `books`、`bookProgress` 和可见的 `mp.book` |
| 批量书籍信息 | `POST /book/infos` | `bookIds:[...]`；读取 `data`，补齐仅有进度记录的书籍 |
| 单本信息 | `GET /book/info` | `bookId`；后续使用 `format/version` |
| 目录 | `POST /book/chapterInfos` | `bookIds:[id], synckeys:[0]`；取 `data[0].updated`（JSON 数组下标） |
| 云端位置 | `GET /book/getProgress` | `bookId`；取顶层 `book`，不是直接读取顶层章节字段 |
| 正文 | `GET /book/chapterdownload` | 见第 5 节 |
| 上报配置 | `GET /feature` | `synckey=0`；取 `feature.guest_token/MaxReadingTime` |
| 阅读上报 | `POST /book/read` | 位置、时长、小时桶和独立签名，见第 6 节 |
| 单本阅读统计 | `GET /book/readinfo` | `bookId, readingDetail=1, readingBookIndex=1` |
| 加入书架 | `POST /shelf/add` | `bookIds:[id], albumIds:[]` |
| 移出书架 | `POST /shelf/delete` | `bookIds, albumIds:[], archiveIds:[]` |
| 私密阅读 | `POST /book/secret` | `bookIds, albumIds:[], private:1/0` |
| 猜你喜欢 | `GET /market/list` | `type=23, synckey=0, count, maxIdx, rn=1`；取 `data[0].books` |
| 榜单目录 | `GET /market/categories` | `ranklist=1, synckey=0, type=0` |
| 榜单书籍 | `GET /market/category` | `categoryId, synckey=0, count, maxIdx, rank=1` |
| 搜索热词 | `GET /searchtags` | 从 `tags` 提取 `tag/originalText` |
| 书籍搜索 | `GET /store/search` | `keyword, count, maxIdx, type=0, outer=0, fromBookId=""` |
| 个人划线/书签 | `GET /book/bookmarklist` | `bookId`；取 `updated` |
| 个人想法 | `GET /review/list` | `bookId, listType=11, mine=1, synckey=0, listMode=0` |
| 全书书评 | `GET /review/list` | `bookId, listType=3, synckey=0, listMode=0, maxIdx, count` |
| 单章热门划线 | `GET /book/underlines` | `bookId, chapterUid, synckey=0`；取 `underlines` |
| 全书热门划线 | `GET /book/bestbookmarks` | `bookId, synckey=0, maxIdx, count`；取 `items` |
| 划线关联想法 | `POST /book/readreviews` | `bookId, chapterUid, cht2sMode:"", reviews:[{range,count,maxIdx,synckey:0}]` |
| 新增划线 | `POST /book/addBookmark` | `bookId, chapterUid, type:1, range, markText, bookVersion, style:0` |
| 新增书签 | `POST /book/addBookmark` | `type:0`，`range` 为位置的十进制字符串，其他字段同上 |
| 删除划线/书签 | `POST /book/removeBookmark` | 此项目传单个 `bookmarkId`，与部分 SDK 的 `bookmarkIds:[id]` 写法不同 |
| 新增想法 | `POST /review/add` | `bookId, chapterUid, range, abstract, content, type:1, bookVersion, isPrivate:1/0, friendship:0, contextAbstract:""` |
| 删除想法 | `POST /review/delete` | `reviewId` |
| 点赞/取消 | `POST /review/like` | `reviewId, isUnlike:0/1` |
| 用户信息/阅读时长 | `GET /user/profile` | `userVid`，并置 `gender/signature/vDesc/totalReadingTime/currentReadingTime/isBlackMe/isInBlackList=1` |
| 会员卡 | `GET /pay/memberCardSummary` | `pf=wechat_wx-2001-android-100-weread` |
| 余额 | `POST /pay/balance` | `pf, zoneId:"1"` |
| 购买章节 | `POST /pay/buyChapters` | `bookId, chapterIds`（逗号分隔字符串）、`isautopay`（字符串）、`totalprice, pf, zoneid, release, payType, isMCard` |

上述接口均有 Lua 调用实现。`listType=11`、单个 `bookmarkId` 等只能证明这个快照采用这种参数，不能据此否定其他 SDK 的变体。会员购买、充值另通过带编码 VID 的手机网页二维码完成，不能把这些页面链接当作阅读 API。来源：[direct.lua][direct]。

书架处理还值得注意：源码把 `bookProgress` 中不在 `books` 的记录视作私密书，批量查信息，再按 `readUpdateTime`、进度 `updateTime`、书籍 `updateTime` 排序。这是其合并策略；仅凭两份列表差集，不足以独立确认所有记录的隐私语义。

### 4.1 weread-omni 的补充接口与参数变体

| 接口 | weread-omni 参数/说明 |
| --- | --- |
| `GET /shelf/sync` | SDK 调用不传业务参数；Lua 显式传两个零游标 |
| `POST /shelf/add` | `bookIds:[id], albumIds:[], archiveIds:[]` |
| `POST /book/chapterInfos` | `bookIds:[id], synckeys:[0], updateTimes:[0], maxfreeIdx:[0]` |
| `GET /store/search` | `keyword, scope`（默认电子书 `10`）、`count, maxIdx` |
| `GET /store/suggest` | `keyword, count` |
| `POST /book/markstatus` | `bookId, status:4, isCancel, auto:0, finishInfo:0`；标记读完，不能替代位置/时长上报 |
| `GET /book/bookmarklist` | `bookId, synckey` |
| `POST /book/updateBookmark` | `bookmarkId, style`，可选 `colorStyle` |
| `POST /book/removeBookmark` | `bookmarkIds:[id]`；Lua 使用单个 `bookmarkId` |
| `GET /review/list` | 个人想法用 `bookId, mine:1, listType:1, listMode:0, synckey, count`；Lua 用 `listType:11` |

章节响应位于 `data[].updated`，SDK 转换后的 `chapters` 不是原始响应字段。`synckey` 是同步游标，不能当作分页 `maxIdx`；`chapterUid` 与 `chapterIdx` 也不能互换。各参数变体需单独联调，不能凭某份源码否定另一份实现。来源：[书籍][native-book]、[书架][native-shelf]、[搜索][native-search]、[笔记][native-notes]。

### 4.2 累计与本周阅读时长

2026-10-27 已接入用户页展示，端点依据 [weread-omni 客户端目录][native-catalog]和[阅读统计实现](https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/read-data.ts)。

| 功能 | 方法/路径 | 本地解析契约 |
| --- | --- | --- |
| 累计阅读时长 | `GET /user/profile?userVid=<vid>&totalReadingTime=1` | 顶层 `totalReadingTime`，按秒转为小时、分钟 |
| 本周阅读时间 | `GET /readdata/detail?mode=weekly&baseTime=<当前 Unix 秒>` | 顶层 `totalReadTime`，按秒转为小时、分钟；以当前时间定位服务端本周统计 |

两项请求携带现有墨水屏客户端版本头、`vid` 和 `accessToken`。登录、恢复会话及书架刷新成功后请求；鉴权过期时合并刷新一次 token 并保存，再重试两项请求。单项缺失、格式错误或网络失败仅将该项显示为“暂不可用”，不会把未知数据当成 0，也不会清除已加载的书架。登出清除页面中的账号数据；当前未持久化用户统计。

这些响应字段及单位已有本地模拟响应和边界测试，真实账号返回结构和服务端周统计边界仍需账号联调，Kindle 真机显示仍待验证。设备时钟应保持正确；每次成功刷新都以当前 Unix 秒重新定位本周，不在本地累加或截取最近七天。

Lua 的 `/user/profile` 还请求 `currentReadingTime`，其注释称仅 `rankWeek=1` 时可视为本周值。这不是本项目已经验证的等价替代；rereader 保持 `/readdata/detail` 路径。

### 4.3 仍只有端点记录的接口

| 方法/路径 | 已记录字段 | 未确认部分 |
| --- | --- | --- |
| `GET /book/download` | `bookId` | 响应、格式与资源关系，不能认定返回完整 EPUB |
| `POST /book/batchUploadProgress` | `books, timestamp, random, signature` 等 | 每条记录结构、签名、幂等与去重语义 |

来源：[客户端端点目录][native-catalog]。不要将目录中有路径等同于存在可用高层实现。

## 5. 正文下载与解包

下载参数为 `bookId`、逗号分隔的 `chapters`、服务端 `version` 作为 `bookVersion`、服务端 `format` 作为 `bookType`，以及 `pf=wechat_wx-2001-android-100-weread`、`pfkey=pfKey`、`zoneId=1`、`quote=""`、`release=1`、`stopAutoPayWhenBNE=1`、`preload=0`、`preview=0`、`offline=0`。源码传了这些字段，不等于每个字段都必填，也不能将 `stopAutoPayWhenBNE` 推广解释为绝对禁止一切扣费。

EPUB 分支：

1. 初次缺少 `Styles` 目录时，下载 UID 0 的公用资源和目标章节。
2. 响应体是加密 ZIP；响应头 `encryptkey` 为经过 AES 包装的 ZIP 密码。
3. 直接循环重复 VID 的 ASCII 字节，取 32 字节；前 16 字节为 AES key，后 16 字节为 IV。
4. 对响应头执行标准 Base64 解码、AES-128-CBC 解密和 PKCS#7 去填充，结果原始字节作为 ZipCrypto 密码。
5. 读取 `info.txt` 中的 `chapters[].files`，关联目标章节的 XHTML 与资源。
6. 生成独立的单章 EPUB，交给 KOReader crengine 排版；不是把 ZIP 二进制直接当文字。

**正文密码派生不使用 REMAP 表。** weread-kindle 的 REMAP 用于第 6 节的阅读上报签名，应与其他项目使用 REMAP 派生正文密钥的做法分开。

TXT 分支按 Content-Type 中的 `tar` 判断无密码归档，读取 `info.txt` 和 `<bookId>_<chapterUid>_o` 文本，再转为 XHTML/单章 EPUB。此分支仅完成源码核对，没有本次 TXT 样本验证。

下载错误 `-2223` 被识别为需要购买，`data.info` 传到试读/购买 UI。没有权限不等于解密失败。来源：[下载层][direct]、[章节准备与单章 EPUB][book]。

样本已验证的密码派生步骤：

```text
v        = VID 的 ASCII 字节
material = 循环重复 v，取前 32 字节
key      = material[0:16]
iv       = material[16:32]
cipher   = 标准 Base64 解码响应头 encryptKey
padded   = AES-128-CBC 解密(cipher, key, iv)
password = 严格 PKCS#7 去填充(padded)
files    = 使用原始 password 字节读取 ZipCrypto 章节 ZIP
```

HTTP 响应头名称不区分大小写；代码应兼容 `encryptKey/encryptkey`。IV 从完整 32 字节缓冲的后半段取得，不能默认等于 key；密码不做 ASCII 筛选，也不把响应头原文或 Base64 解码结果直接当 ZIP 密码。AES 解的是密码包装，不是整个 ZIP。

下载层必须先区分 HTTP/JSON 错误和归档响应。解包成功应验证目标条目完整读取和 CRC、`info.txt` 中的书籍/章节身份及文件关联、XHTML 结构，再提交正文缓存。若落盘，限制条目数、解压总大小，拒绝绝对路径、`..`、重复条目及符号链接逃逸。

已验证样本无需额外调用 `getPublishCertificate`；此前检索没有找到可核对的相关定义，不能根据名称推测路径，也不能推广断言所有版本都不需要它。

## 6. 阅读进度与时长签名

这是比 Python 原型更完整的部分。源码注释称算法还原自墨水屏 `libencrypt.so`，并称作者在 Mac 上观察到时长增加；本项目尚未独立验证这项上报实测。

配置：`GET /feature?synckey=0`，取 `feature.guest_token`，本地缓存一天；同时保存 `feature.MaxReadingTime`，缺失时计时上限默认 130 秒。

签名与登录 SHA256 拼接算法不同：

```text
payload = 用下划线连接：
  vid, deviceId, deviceId, bookId, 0, 时区, readingTime, 0,
  [每个小时桶的 startOfHour, timeZone, readingTime, ttsTime]

inputs = [guest_token, random 的十进制字符串, timestamp 的十进制字符串, payload]
对每个 input 的原始字节逐字节做 REMAP[0..255] 替换
追加未映射的常量字符串 "5a6f1"
按字节字典序排列这些字符串，再无分隔拼接

rotateSha(s):
  k = 所有字节 XOR 的结果 % 11
  将整个字节序列循环右移 k 个字节
  SHA256，输出小写十六进制字符串

signature = rotateSha(rotateSha(拼接结果))
```

REMAP 的 256 字节常量（按连续十六进制解码，下标 0–255）：

```text
34ca55401db693c63130293532a7b811c2b516fa8bb124a4109004e908f83b8a
9c8c44f9bc5c69e2a1dad2d37589f71e2d5056d77253bf22fb200f012e45876e
6648f2e0cdfe67a943f49451cea54aee13268eccaa33145d0e39bbcf912b814d
ea99ec1a2c85c5d936744b18e1f13d9d419fb4170dd64cbedcaf972877f062ff
71c1c8278f6c68a89be6591c1b1209984e3f063700ba1f0a192fc9d5d057496f
fd25e4610c42cb96645fdbad60238d9a6dc3c45e3eb9926abd5b077f7695ed4f
ab847a80e778c7e5eb73836bfc38467d4765b352633a05d1efa3a6de9e3c02ae
b27ba0f6f32ac0ac86035a540bf582d47ee3dfb0d8dd21e87c88a2795870b715
```

源码见 [`direct.lua` 的 `REMAP_HEX/genSignature`][signature]。Rust 实现应使用字节缓冲，不能把映射后的任意二进制字节当 UTF-8 字符串排序；第二轮哈希处理第一轮的十六进制文本。

`/book/read` 的 `timestamp` 取 `util.gettime()` 首返回值（秒），登录防重放时间戳则是毫秒，不能共用当前登录签名函数。随机数仍为 0–999。

请求字段包括：

- 位置：`bookId, bookVersion, chapterUid, chapterIdx, chapterOffset, progress, summary`。
- 时长：`readingTime` 和 `hours:[{startOfHour,timeZone,readingTime,ttsTime}]`，总时长为桶内秒数之和。
- 身份/签名：`deviceId, appId, installId` 均取设备 ID；另传 `timestamp, random, signature, risk:0, recordCreateTimeZone`。
- 其他固定字段：`reviewId:""`；`ttsTime/lectureTime/lectureTextTime/novalTime/chapterProgress/isLecture/voiceType/autoTime/isStoryFeed/wordCount` 均置 0，注意原字段拼写 `novalTime`。

阅读页每次操作累计距上次操作的秒数，上限取 `MaxReadingTime`，放入当前小时桶。翻页后延迟 10 秒上报，连续翻页重置延迟；退出也尝试上报。后台上报前取走当前桶，失败时合并回内存。

这些调用策略不是服务端保证：跨小时区间全部放入操作发生时的桶；失败数据没有形成持久化同步队列；请求已被服务端处理但响应丢失时，重报可能重复累计。Rust 接入仍需明确幂等/去重策略与官方客户端读回结果。

**位置是估算值。** 阅读页实际计算 `floor(chapter.wordCount * (page - 1) / pageCount)` 作为 `chapterOffset`，全书百分比也按章节字数和页比例估算。不能据此声称已确定精确章内偏移单位或完成原文定位。来源：[上报签名][signature]、[阅读页计时与上报][report]。

## 7. 笔记定位与原文偏移

`book.lua` 按原始 XHTML 建立可见文字到源文件字符位置的映射，扫描时也计入标签和换行占用的位置；实体保留其原始编码长度。划线用 `起-止` 字符串，书签用单个位置字符串。重复文字通过选区在章节内的大致比例挑最近匹配。

具体实现按 UTF-8 序列逐个计数，起点从 0 开始，终点由匹配末字符的结束位置产生；这是源码行为，不能据此确认服务端在 emoji、UTF-16 代理对、组合字符等情况下使用完全相同的单位。原始文件的 CRLF 不应在定位前随意归一化。其 `chapterSource()` 只取章节 `files` 的第一个文件，多文件章节和 TXT 转换后的原文坐标也需要另测。来源：[plainIndex/findRange][range]。

## 8. 传输、错误与重试

[mobile.ts][transport] 分开提供 `call()` 和 `callRaw()`：前者解析 JSON，后者返回字节。二者都识别业务错误；字节响应若实际是 token 过期的 JSON，不能缓存成电子书。

| 情况 | 参考实现行为 |
| --- | --- |
| HTTP 401 或业务码 `-2012` | 按会话失效处理；可重放请求最多重试一次 |
| GET/HEAD | 默认可重放 |
| 只读 POST，如目录 | 资源方法显式设置 `idempotent: true` |
| 非幂等写入遇鉴权失败 | 可刷新供后续调用使用，但不自动重放当前写入；结果可能不确定 |
| 业务码 `-2041` | 归为人机验证要求 |
| 业务错误 JSON | 同时兼容 `errCode`/`errcode` 和对应消息字段 |
| 重定向 | `redirect: "error"`，不自动跟随 |
| 原始字节下载 | 可设置 `maxResponseBytes`；调用方仍需校验真实格式 |

尤其需要注意：`callRaw()` 只是下载传输能力，没有实现解包、解码或解密。它只接受当前 API 基地址内的路径；如果下载响应返回 `res.weread.qq.com` 的 URL，需要单独确认下载主机、认证和跳转策略，不能盲目转发 API 身份头。

错误分类还包括参考 SDK 的 `-2014` 限流，以及 Lua 识别的 `-2010` 缺少 VID、`-2013` 鉴权失败、`-2223` 章节需购买。除第 9 节的 `-2010` 对照外，这些语义来自源码，仍应结合具体接口响应确认。

weread-kindle 的以下行为不能直接照搬：

- 通用 JSON API 的鉴权重试有一次上限，但下载函数在刷新成功后递归重试，未设置同样的上限。
- HTTP 层在网络失败时对包括 POST 在内的请求重试，不能直接用于要求避免重复的想法、购买和时长写入。
- JSON API 主要依赖 `errcode`，没有完整验证 HTTP 成功状态；应同时检查 HTTP、业务错误与响应结构。
- 通用下载临时路径固定为 `/tmp/weread_chapter.zip`，多任务共享时存在覆盖可能；缓存路径按书籍区分，未按账号隔离。
- `guestToken()` 拉取失败时允许回退到旧 token 或空字符串，应在本项目显式处理不可用状态。

## 9. 已有实测与复测方式

### 9.1 2026-10-27 样本记录

样本为书籍 `832038`、章节 UID `86`（免费前言），版本 `1495733324`；书籍信息标记已购。保持应用原有墨水屏请求头，不额外填写 `pf/pfkey`。

| 检查 | 实测结果 |
| --- | --- |
| 新下载 | HTTP 200，`application/zip`，4349 字节 |
| 密钥包装 | Base64 解码为 16 字节，AES 解密后严格 PKCS#7 校验通过 |
| `info.txt` | 264 字节，完整读取、CRC 和 JSON 解析通过 |
| 内容身份 | bookId 匹配，chapters 中 UID 86 匹配 |
| 内容资源关联 | `chapters[].files` 指向 `Text/Section0001.xhtml` |
| 章节 XHTML | 7809 字节，完整读取、CRC 和 XML 解析通过 |
| 文本检查 | XML `itertext()` 合计 2433 字符，含文档内空白及其他文本节点，不作为平台字数 |
| 无凭据对照 | HTTP 401，`errcode=-2010` |
| 离线拒绝路径 | 错误 Base64、错误书籍身份、错误章节身份、合法 AES 包装但错误 ZIP 密码均被拒绝 |

旧的数字映射方法失败，是因为它未通过此样本验证。新方法通过实测后，可确定该样本需要使用未映射的 VID 字节；不能推断所有客户端版本或书籍格式都采用相同处理。

既有记录使用应用 SQLite 活动会话、原有墨水屏请求头，没有修改或刷新凭据，也没有购买或上报阅读数据。请求链路为 `/shelf/sync → /book/info → /book/chapterInfos → /book/chapterdownload`，前三项 HTTP 200，书架当时返回 199 本书；这些是历史样本结果，不代表当前账号数据。

下载请求的实测参数：

```text
GET /book/chapterdownload?bookId=832038&bookType=epub&bookVersion=1495733324&chapters=86&stopAutoPayWhenBNE=1
```

未带 `pf/pfkey/zoneId/offline/preload/preview` 仍成功，仅证明这些字段对此样本可省略。书籍《道德经：百万读者，过万好评（全注全译典藏版）》标记 `paid=1`；前言 UID 86 的目录序号为 3、`price=0`。目录 synckey 恰好与版本相同，不能视为通用等价关系。

ZIP 两条目均为 Deflate（method 8），设置传统加密标记，无 WinZip AES extra field `0x9901`；响应头值长 24 字符（不记录其值）。重复下载 ZIP 的 SHA-256 不同，不以整包字节恒定判断复测成功。最初仅验证 ZIP 结构的阶段结论，已被后续 CRC/XHTML 验证补全。

### 9.2 复测脚本

已为 `scripts/probe-eink-download.py` 增加显式的 `--verify-plaintext`。默认仍只检查下载包；启用后使用 `cryptography` 在内存解密并检查包内文件，报告不包含正文或密钥。

```sh
python3 -m venv /tmp/rereader-content-verification-venv
/tmp/rereader-content-verification-venv/bin/python -m pip install cryptography
/tmp/rereader-content-verification-venv/bin/python scripts/probe-eink-download.py \
  --book-id 832038 --chapter-uid 86 --verify-plaintext --auth-control
```

成功报告应包含 `plaintext_verified=true`、书籍/章节身份匹配、`crc_verified_entries=2` 和 XML 验证结果。只在显式传 `--archive` 时保存原始加密 ZIP，不会写出解包明文。

脚本限制包内条目数、解压总大小，拒绝重复或危险路径；完整读到条目结尾以检查 CRC。若 key 包装、身份、资源关联或 XML 解析失败，返回错误，不宣称正文可用。

仅检查下载结构可运行：

```sh
python3 scripts/probe-eink-download.py --book-id 832038 --chapter-uid 86 --auth-control
```

默认成功只表示可识别 ZIP，`plaintext_verified=false`；只有显式启用解包且全部校验通过才是 `true`。脚本只读打开账号数据库，不刷新 token、不自动重试、不跟随重定向，不打印凭据、响应正文或密钥。数据目录沿用 `REREADER_DATA_DIR`，可用 `--data-dir` 覆盖。

仅覆盖服务端提供版本号的 EPUB，目标章节须有明确免费或已购标记，不推断会员权限。省略章节 UID 时选择首个 `price=0` 且 `wordCount>=500` 的章节。`--archive` 以独占创建、权限 `0600` 保存原始 ZIP，不保存 encryptKey；样本和账号响应不提交仓库。

验证仍仅限一本 EPUB 的一章。会员/未购章节、TXT、PDF、插图加载、其他账号、其他客户端版本、原文定位和云端进度写回均未验证。

## 10. 旧算法对照

旧 weread_downloader 方案将 VID 的每位数字映射后再派生 AES 参数，本项目旧样本和新下载同章样本均未通过。保留这项对照，是为了避免把其他客户端版本的密码派生方法混入当前已验证链路。

源码将 VID 转为十进制字符串，逐位查一个 10 字节的表：

| 十进制数字 | 映射字节（十六进制） |
| --- | --- |
| 0 | 2d |
| 1 | 50 |
| 2 | 56 |
| 3 | d7 |
| 4 | 72 |
| 5 | 53 |
| 6 | bf |
| 7 | 22 |
| 8 | fb |
| 9 | 20 |

把映射结果循环重复到 32 字节，前 16 字节作为 AES key，后 16 字节作为 CBC IV。

这里使用的是映射后的原始字节，不是这些字节的十六进制文本；IV 也不是一律复制 key。只有重复模式恰好对齐时，两段才可能相等。

上游先把一个容量为 32 的 Go slice 缩短为长度 16，再通过 `[16:32]` 取 IV。Go 允许在容量内重新切片，因此这段写法本身不是越界 bug；移植到 Rust/Python 时，应显式从原始 32 字节缓冲分别取两段。

旧实现对 Base64 响应头做 AES-CBC 解密后保留 `0x20`–`0x7e` 字节作为密码，未严格执行 PKCS#7；还固定分配 16 字节输出缓冲，不能视为通用多块实现。其 `MergePdfBook` 实际在 EPUB 分支合并 XHTML 为 HTML，没有生成 PDF 或重建完整 EPUB。

测试只使用已购书 `832038` 的免费前言 UID `86`；参数与第 9 节一致。

| 检查 | 结果与能证明的范围 |
| --- | --- |
| 旧 ZIP 与原配响应头 | Base64 可解码为 16 字节；按上游算法生成密码后，`info.txt` 报 `Bad password` |
| 新下载同一章及其原配响应头 | 保持墨水屏 2.1.2 头，HTTP 200、4349 字节 ZIP；同样报密码错误 |
| AES 实验实现自检 | NIST AES-128-CBC 已知向量通过，排除该向量下 AES 调用或 key/IV 接口使用错误 |
| 严格 PKCS#7 诊断 | 两份墨水屏样本的派生解密结果均不满足；但上游使用 ASCII 筛选，所以单凭此项不能判定上游流程失败 |
| 上游 ASCII 筛选兼容行为 | 已按源码复现，仍无法打开 `info.txt`，未进入正文 CRC/XML 校验阶段 |
| 切换上游 Android 7.5.0 请求头配置 | 保持同一 accessToken、同章下载参数，返回 HTTP 401；没有继续尝试登录或刷新 |

实验只输出状态、大小、校验结论；没有输出 VID、token、AES key/IV、ZIP 密码、encryptKey 值或正文。既有记录中的原始下载样本和响应头曾保存到 `/tmp` 下权限为 `0700` 的研究目录，文件权限为 `0600`，不加入仓库；临时文件现在是否仍存在不作为协议证据。

目前只能确定“这份参考算法未通过当前样本验证”。映射/派生方式是否随客户端版本改变、服务端是否采用另一种密钥包装、或还有未识别的上下文依赖，都需要新的证据。不能直接将失败归因为某一种原因。

## 11. 本项目接入与验证

网络客户端位于 `src/weread/`，Slint 任务生命周期位于 `src/app.rs`，账号与正文/笔记/进度存储位于 `src/storage/`。存储契约见 [SQLite 与本地文件设计](storage-design.md)，运行方式和当前 UI 范围见 [README](../README.md#扫码登录与书架)。

网络、解包、内容解析和磁盘操作在工作线程执行，通过 Slint 事件循环更新 UI；桌面与 Kindle 共享数据层。

1. 基于已验证单章路径接入目录、服务端版本、UID 0 公用资源、XHTML 和插图；另测 TXT、会员/未购、试读终点及下架场景。
2. 确认 `bookProgress` 差集补查 `/book/infos` 的真实结构与隐私语义。
3. 为 `/feature` 与 `/book/read` 建独立签名实现；先确认零时长位置更新，再用少量真实前台阅读时间验证累计值与官方客户端恢复位置。
4. 保留原始 XHTML、内容版本和源位置映射，测试中文、CRLF、实体、emoji、组合字符与多文件章节。
5. 使用持久化同步队列处理断网、重启、账号切换与多设备冲突；时长增量不能盲目重放。
6. 先读回划线，再验证新增、更新和删除；明确 `listType` 与 `bookmarkId/bookmarkIds` 的参数变体。

weread-omni 的登录/传输测试主要使用注入响应；在线测试默认跳过，公开回放主要覆盖搜索元数据。这些不能证明正文或进度写回可用。本次文档合并仅做结构、链接及内容一致性检查，没有重跑账号实测或 Kindle 真机验证。

## 12. 固定来源

| 来源 | 固定提交 | 用途 |
| --- | --- | --- |
| [teng-lin/weread-omni][native-profile] | `88bd2e095d7d7ee423eaadf8f40653e72c5be6d4` | 登录、刷新、传输、资源 API、端点目录；仓库版本 0.1.2，Node.js ≥22.13.0，不能等同于 npm 产物 |
| [Zz2217967571/weread-kindle][direct] | `2d9c8d6775786055f2955fbc1532a525f8067dd1` | Python 原型、Lua 网络层、正文、进度签名、笔记定位 |
| [HuanMeng233/weread_downloader][old-decrypt] | `1399369a4910ec72e9850365c82bbfc87b299323` | 数字映射密码派生对照；核心 Git blob `659a3524ecfb74d7d997ffa8aac82e379bf93675` |
| [AnkioTomas/moon][moon-package] | `0912b10a8df779d33a6c2d182563bedf2e262359` | REMAP 密码派生对照 |

weread-kindle 的 Python 原型没有进度上报方法，完整上报证据在 Lua；weread-omni 的 `ChapterContent` 类型和 `callRaw()` 字节传输能力也不等于完整正文读取实现。源码称部分算法来自墨水屏 APK，本项目没有独立重做 APK 反编译。

移植第三方实现时保留相应许可和作者声明，参见项目 `THIRD_PARTY_NOTICES.md`。

[native-login]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/qrlogin.ts
[native-token]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/token.ts
[native-profile]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/profile.ts
[native-device]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/device-ua.ts
[native-book]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/book.ts
[native-shelf]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/shelf.ts
[native-search]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/search.ts
[native-notes]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/notes.ts
[native-catalog]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/docs/endpoints.md
[direct]: https://github.com/Zz2217967571/weread-kindle/blob/2d9c8d6775786055f2955fbc1532a525f8067dd1/kindle/weread/direct.lua
[book]: https://github.com/Zz2217967571/weread-kindle/blob/2d9c8d6775786055f2955fbc1532a525f8067dd1/kindle/weread/book.lua
[signature]: https://github.com/Zz2217967571/weread-kindle/blob/2d9c8d6775786055f2955fbc1532a525f8067dd1/kindle/weread/direct.lua#L337-L423
[report]: https://github.com/Zz2217967571/weread-kindle/blob/2d9c8d6775786055f2955fbc1532a525f8067dd1/kindle/weread/readerview.lua#L676-L740
[range]: https://github.com/Zz2217967571/weread-kindle/blob/2d9c8d6775786055f2955fbc1532a525f8067dd1/kindle/weread/book.lua#L445-L583
[credentials]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/credentials.ts
[transport]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/mobile.ts
[old-decrypt]: https://github.com/HuanMeng233/weread_downloader/blob/1399369a4910ec72e9850365c82bbfc87b299323/decrypt/weread.go
[moon-package]: https://github.com/AnkioTomas/moon/blob/0912b10a8df779d33a6c2d182563bedf2e262359/book.koplugin/source/wechat/package.lua
