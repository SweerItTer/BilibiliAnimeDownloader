import requests, re
import os
from pprint import pprint


def get_apidata_list(cartoon_ss_num, sessions):
    headers = {
        "Accept": "application/json, text/plain, */*",
        'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num,  # 什么?B站防盗链?就这',
        'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36',
    }

    ep_id = cartoon_ss_num[2:]
    response = sessions.get(url=f'https://api.bilibili.com/pgc/view/web/season?ep_id={ep_id}', headers=headers)
    jsondata = response.json()

    return jsondata['result']['episodes']


def transmit_id(title, cartoon_ss_num):
    headers = {
        'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num,
        'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) '
                      'Chrome/117.0.0.0 Safari/537.36',
    }
    rsp = requests.get(url="https://www.bilibili.com/bangumi/play/%s/" % cartoon_ss_num, headers=headers).text
    if cartoon_ss_num[:2] == "ss":
        cartoon_ss_num = "ep" + re.findall('//www.bilibili.com/bangumi/play/ep(.*?)"/>', rsp)[0]
        # pprint(cartoon_ss_num)
    if not os.path.exists(title) and title != "":
        os.makedirs(title)

    return cartoon_ss_num


if __name__ == '__main__':
    cartoon_ss_num = "ep779775"
    cartoon_ss_num = transmit_id("test", cartoon_ss_num)
    # pprint(get_apidata_list(cartoon_ss_num))

    animelist = get_apidata_list(cartoon_ss_num)
    is_member = True
    list_ = []
    for one in animelist:
        if is_member:
            if one["badge"] == "预告":
                continue

            list_.append(
                {"Episode": one["share_copy"],
                 "ids": {
                     "cid": one["cid"],
                     "aid": one["aid"],
                     "ep_id": one["ep_id"],
                 }
                 }
            )

    pprint(list_)
