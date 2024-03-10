import http.cookiejar
import requests
import os
from time import sleep
import qrcode


def get_qrurl() -> list:
    """返回qrcode链接以及token"""

    headers = {
        'User-Agent': 'Mozilla/5.0 (Windows NT 6.1; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36 QIHU 360SE'
    }
    url = 'https://passport.bilibili.com/x/passport-login/web/qrcode/generate?source=main-fe-header'
    returns = requests.get(url=url, headers=headers)
    total_data = returns.json()
    qrcode_url = total_data['data']['url']
    qrcode_key = total_data['data']['qrcode_key']
    data = {'url': qrcode_url, 'qrcode_key': qrcode_key}
    return data


def make_qrcode(data):
    """制作二维码"""
    qr = qrcode.QRCode(
        version=5,
        error_correction=qrcode.constants.ERROR_CORRECT_L,
        box_size=10,
        border=4,
    )
    qr.add_data(data['url'])
    qr.make(fit=True)
    # fill_color和back_color分别控制前景颜色和背景颜色，支持输入RGB色，注意颜色更改可能会导致二维码扫描识别失败
    img = qr.make_image(fill_color="black")
    try:
        img.save('./temp/Qr.png')
    except:
        os.mkdir('temp')
        img.save('./temp/Qr.png')


def judge(data) -> dict:
    """主函数"""
    token = data['qrcode_key']
    sessions = requests.session()
    headers = {
        'User-Agent': 'Mozilla/5.0 (Windows NT 6.1; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36 QIHU 360SE'
    }
    url = f"https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key={token}&source=main-fe-header"
    sessions.cookies = http.cookiejar.LWPCookieJar(filename='./temp/cookies.txt')
    data_login = sessions.get(url=url, headers=headers)  # 请求二维码状态
    status = data_login.json()

    code = int(status['data']['code'])
    if code == 0:
        sessions.cookies.save(ignore_discard=True, ignore_expires=True)
    return code


if __name__ == "__main__":
    data = get_qrurl()
    make_qrcode(data)
    while True:
        code = judge(data)
        if code == 0:
            break
        sleep(1)