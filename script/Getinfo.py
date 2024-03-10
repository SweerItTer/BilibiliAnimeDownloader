from re import findall as fa
from requests import get

def getinfo(epid):

    if "ep" not in epid:
        epid = f"ep{epid}"


    headers = {
        "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36"
    }
    homeurl = f"https://www.bilibili.com/bangumi/play/{epid}"

    try:
        resp = get(url=homeurl, headers=headers).text
    except Exception as e:
        print(f"请求失败: {e}")
        return None, None

    cover_matches = fa('<img style="width:165px;height:221px" src="(.*?)"', resp)
    title_matches = fa('class="mediainfo_mediaTitle__Zyiqh" target="_blank" rel="noreferrer" title="(.*?)"', resp)

    if not cover_matches or not title_matches:
        print("封面或标题信息未找到")
        return None, None

    coverurl = cover_matches[0]
    if not coverurl:
        print("封面URL无效")
        return None, None

    coverurl = f"https:{coverurl}@310w_422h.webp"

    try:
        resp_con = get(url=coverurl, headers=headers, stream=True)
        filepath = f"./temp/{epid}.png"
        with open(filepath, "wb") as pic:
            for chunk in resp_con.iter_content(chunk_size=10240):
                pic.write(chunk)
    except Exception as e:
        print(f"保存图片文件时出错: {e}")
        return None, None

    title = title_matches[0]

    if not title:
        print("标题信息无效")
        return None, None
    else:
        illegal_list = [' ', '\\', '/', ':', '*', '?', '<', '>', '"', '|', '.']

        for char in illegal_list:
            title = title.replace(char, '-')

    return filepath, title


if __name__ == "__main__":
    getinfo("779775")
