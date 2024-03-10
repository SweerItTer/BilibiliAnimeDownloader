
def getcover(epid):
    from re import findall as fa
    from requests import get

    if "ep" not in epid:
        epid = f"ep{epid}"

    headers = {
        "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36"
    }
    homeurl = f"https://www.bilibili.com/bangumi/play/{epid}"

    resp = get(url=homeurl, headers=headers).text

    coverurl = fa('<img style="width:165px;height:221px" src="(.*?)"', resp)[0]
    if coverurl:
        coverurl = f"https:{coverurl}@310w_422h.webp"
        resp_con = get(url=coverurl, headers=headers, stream=True)
        filepath = f"temp/{epid}.png"
        with open(filepath, "wb") as pic:
            for chunk in resp_con.iter_content(chunk_size=10240):
                pic.write(chunk)
        return filepath
    else:
        return None
    # print(coverurl)


if __name__ == "__main__":

    getcover("779775")