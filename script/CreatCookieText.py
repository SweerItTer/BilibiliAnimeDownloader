import http.cookiejar

cookies_str = "buvid3=AA1C2AAC-4EB4-B3FC-7207-9B405616895981968infoc; b_nut=1705850381; _uuid=6A1092B3A-5262-52EB-4F15-F89D459F4BD381900infoc; buvid4=BE5A7FD8-7166-1D3E-A867-32ACC5A8D63E82703-024012115-Usbid0KJc2NWzQG2bR2cAA%3D%3D; rpdid=|(k|k)ulRlk)0J'u~|l)l|J)J; DedeUserID=97108444; DedeUserID__ckMd5=f4d0acb52b9cba3a; buvid_fp_plain=undefined; enable_web_push=DISABLE; header_theme_version=CLOSE; home_feed_column=5; hit-dyn-v2=1; LIVE_BUVID=AUTO8217061109344749; CURRENT_BLACKGAP=0; CURRENT_QUALITY=112; PVID=1; is-2022-channel=1; FEED_LIVE_VERSION=V_FAVOR_WATCH_LATER; bp_video_offset_97108444=903986127876128768; fingerprint=a4cea821b48a4eb63f7dab93650c7c13; browser_resolution=1601-931; CURRENT_FNVAL=4048; buvid_fp=a4cea821b48a4eb63f7dab93650c7c13; SESSDATA=6bbaa536%2C1725462890%2C1f3ba%2A31CjBVHjfaVhpKCTPkPa55S4HYJiJuO8Rn0YI4hVkiCsMWWwemZzsDyxqJ1BrU7N4DMysSVjZwRXNjRm14QjkzWHNLdkQ5ZXkzYU5uZGJoYUxhQTFHWGotRlBuOUNyNWwwdUVMQVhUaVpNdTFCVmwxM0R6SFJ3NkY5RG9hZmRDdENnbWNlTkVKNkZRIIEC; bili_jct=74cbee6059043f63eddbfc71a95eb458; bili_ticket=eyJhbGciOiJIUzI1NiIsImtpZCI6InMwMyIsInR5cCI6IkpXVCJ9.eyJleHAiOjE3MTAxNzAwOTksImlhdCI6MTcwOTkxMDgzOSwicGx0IjotMX0.B_RA7QWBZhxgxgJDYMa2IrOLtmw_a92XAXifUcmvdaM; bili_ticket_expires=1710170039; b_lsid=837A421010_18E1F19C556; sid=p4o2owq8"

# 将 cookies 字符串转换成字典格式
cookies_dict = {}
for cookie in cookies_str.split("; "):
    name, value = cookie.split("=")
    cookies_dict[name] = value
# 创建 LWPCookieJar 对象
cookies = http.cookiejar.LWPCookieJar()

# 将 cookies 添加到 LWPCookieJar 对象中
for name, value in cookies_dict.items():
    cookies.set_cookie(http.cookiejar.Cookie(
        version=0,
        name=name,
        value=value,
        port=None,
        port_specified=False,
        domain="",
        domain_specified=False,
        domain_initial_dot=False,
        path="/",
        path_specified=True,
        secure=False,
        expires=None,
        discard=False,
        comment=None,
        comment_url=None,
        rest={"HttpOnly": None},
        rfc2109=False
    ))

# 保存 cookies 到文件
cookies.save("../cookies.txt")