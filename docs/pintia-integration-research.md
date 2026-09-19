# 拼题A (PTA / pintia.cn) 登录与作业抓取调研

调研日期:2026-09-19。信息来源:8 个开源项目的源码逐个核对 + 对 pintia.cn / passport.pintia.cn 的实测(curl + 浏览器 + 前端 bundle 逆向)。

## TL;DR

- 登录走独立认证域 `passport.pintia.cn`,**与浙大统一身份认证 (zjuam CAS) 无关**;`/auth/student-login` 已 404,目前不存在学校 SSO 通道。
- 唯一凭证是登录响应 `Set-Cookie` 下发的 **`PTASession` cookie**(不是 Bearer/JWT),后续所有 `pintia.cn/api/*` 请求带 `Cookie: PTASession=...` 即可。
- 作业列表就是"题集"(problem sets):`GET https://pintia.cn/api/problem-sets?filter={"endAtAfter":"<now ISO8601 UTC>"}` 一次拿到所有未截止的作业/考试,字段里有截止时间 `endAt`、课程组织名 `organizationName`、类型 `type`。
- 密码登录可能触发腾讯防水墙验证码(`GATEWAY_WRONG_CAPTCHA`),无法纯后端兜底,需要"手动粘贴 Cookie"作为降级方案。

## 1. 登录

### 接口(已从前端 bundle 的 API 定义表实测确认)

```
POST https://passport.pintia.cn/api/users/sessions
Content-Type: application/json
Origin: https://pintia.cn
Referer: https://pintia.cn/

{
  "email": "xxx@example.com",     // 或 "phone": "..."(二选一,含 @ 走 email)
  "password": "<明文,无需哈希>",
  "rememberMe": true,             // true 可延长会话有效期
  "inMiniProgram": false,
  "ticket": "<可选,腾讯验证码 ticket>",
  "randStr": "<可选,腾讯验证码 randstr>"
}
```

- 成功:HTTP 200,响应 `Set-Cookie: PTASession=<value>; ...`。**没有 body token**,从 Set-Cookie / cookie jar 提取。
- 密码是**明文**(HTTPS 传输)。所有核查过的现行开源项目(vscode-pintia、ZJU-Academic、pta_crawler、pypintia_utils)都是明文,无 sha1。
- 失败:`{"error":{"code":"...","message":"..."}}`。
  - `GATEWAY_WRONG_CAPTCHA`(HTTP 400)= 需要验证码 → 见下。
  - 415 = 密码错误(pta_crawler 经验)。

### 验证码

- 腾讯防水墙 TCaptcha,lazily 从 `https://turing.captcha.qcloud.com/TCaptcha.js` 加载,appId `194593025`(已从 bundle 实测确认)。
- 前端流程:`new TencentCaptcha(appId, callback)` → 用户滑块 → callback 拿 `{ticket, randstr}` → 随登录请求一起提交。
- **触发规律不确定**:干净 IP 直连 POST 常常不要验证码(pta_crawler 就是裸 requests 登录成功);风控触发后返回 `GATEWAY_WRONG_CAPTCHA`。桌面应用无法内嵌滑块,只能降级。

### 会话校验(实测确认)

```
GET https://passport.pintia.cn/api/u/current
→ 未登录:200 + {"emailLogin":false,"omsLogin":false,"studentUserLogin":false,"phoneLogin":false,"userMfa":[]}
→ 已登录:200 + 完整用户信息(含 username/userId 等)
```

完美的低成本探针:登录后调一次即可判断 PTASession 是否仍有效。

### 其他登录通道(bundle API 表里发现,未深挖)

- 微信扫码 OAuth:`GET /api/oauth/wechat/official-account/auth-url` → 轮询 `state` → `POST /api/users/sessions/state/{state}/login_users/{userId}`(vscode-pintia 有完整实现)。
- 学校组织学生登录:`POST /api/student-users/sessions`,body `[organizationId, organizationCode, studentNumber, name, password, omsExamId, omsClientId]`(主要面向 OMS 考试机)。
- MFA 短信:`POST /api/users/mfa/{uuid}/sms`。

## 2. 作业 / 题集列表(核心数据源)

```
GET https://pintia.cn/api/problem-sets?filter={"endAtAfter":"2026-09-19T00:00:00Z"}&limit=100&order_by=END_AT&asc=true
Cookie: PTASession=...
```

- `filter={"endAtAfter": <now UTC>}` = 只要未截止的;`filter={}` = 全部。
- 响应:`{"problemSets": [...]}`,未登录返回 404 + `USER_NOT_FOUND`(实测)。
- 题集对象字段(实测 always-available 端点 + 开源项目):

```jsonc
{
  "id": "12",
  "name": "浙大版《C语言程序设计(第3版)》题目集",
  "type": "BOOK",            // 题集类型(HOMEWORK/EXAM/BOOK/CONTEST 等,靠此字段区分作业与考试)
  "timeType": "ALWAYS_AVAILABLE",
  "status": "PENDING",
  "organizationName": "拼题A",   // 课程/组织名 → 当"课程名"用
  "ownerNickname": "admin",
  "manageable": false,
  "createAt": "...", "updateAt": "...",
  "startAt": "...", "endAt": "..."   // 作业时间窗,endAt 即截止时间
}
```

### 每个题集的进度 / 完成状态

```
GET /api/problem-sets/{id}/exams                 # 进入题集(POST 同路径=开始考试,重复调返回 412 可忽略)
GET /api/problem-sets/{id}/exam-problem-status   # 自己各题的做题状态
GET /api/problem-sets/{id}/last-submissions      # 自己最近一次提交
GET /api/problem-sets/{id}/problem-summaries     # 各题型数量汇总
```

作业 URL 形如 `https://pintia.cn/problem-sets/{id}/exam/problems`,可以直接从 app 里打开(已接入 opener 插件)。

## 3. 频率限制

- HTTP 429 / 响应体错误码 `RATE_LIMIT_EXCEEDED`。
- 安全间隔经验值:**≥ 600ms/请求**(vscode-pintia 踩坑值;0.2s 是 pypintia 的下限)。命中后睡 3s 重试即可。
- 同步作业列表场景每次只调 1 个接口,基本不会触碰限制。

## 4. 集成建议(zju-learning-assistant)

1. **认证**:新增 PTA 凭据配置(邮箱/手机号 + 密码),登录后把 `PTASession` 存 keyring(与现有 zju 凭据同模式);`GET /api/u/current` 做会话探针,失效自动重登,重登遇 `GATEWAY_WRONG_CAPTCHA` 时提示用户走"手动粘贴 Cookie"降级(设置页加一个输入框,同 vscode-pintia 的做法)。
2. **数据模型**:PTA 作业 → 现有 Todo 面板新增一个来源(tag 区分"学在浙大"/"PTA");字段映射:`name`→标题、`organizationName`→课程、`endAt`→截止时间、`problemSet id`→跳转链接。可选:`exam-problem-status` 汇总"已过题数/总题数"作为完成度。
3. **同步**:挂进现有 `sync_todo_once` 定时同步管线(注意 PTA 请求间隔 ≥0.6s;一次列表请求即可,不用逐题集查详情)。
4. **密码明文提交**只在 HTTPS 下发生,存储侧用 keyring,与现有 zju 密码同等级别。

## 5. 主要参考项目

| 项目 | 语言 | 价值 |
|---|---|---|
| [jinzcdev/vscode-pintia](https://github.com/jinzcdev/vscode-pintia) | TS | 最完整 API 封装;Cookie 手动登录;600ms 限速经验 |
| [AoiKJuice/ZJU-Academic](https://github.com/AoiKJuice/ZJU-Academic) | Python | ZJU 生态 + PTA 密码登录 + PTA 待办聚合(与本需求最接近) |
| [Long17369/pta_crawler](https://github.com/Long17369/pta_crawler) | Python | 裸 requests 密码登录、验证码错误码回落浏览器 |
| [verssionhack/pypintia_utils](https://github.com/verssionhack/pypintia_utils) | Python | 限速处理最细(0.2s 下限/睡 3s) |
| [Dichgrem/Pintia_to_md](https://github.com/Dichgrem/Pintia_to_md) | TS | 纯 PTASession cookie 用法的最小示例 |
