import json
import os
import re
from pprint import pprint

import requests


def fileDownload(url, name, seps):
	print("-----------Downloading------------")

	# 发送option请求服务器分配资源
	session = requests.session()

	session.options(url=url, headers=headers, verify=False)

	res = session.get(url=url, headers=headers, verify=False, timeout = 50, stream=True)

	with open(name+seps, 'wb') as fp:
		for chunk in res.iter_content(chunk_size=1310720):  # 逐块写入，每次写入10mb
			if chunk:
				fp.write(chunk)
				fp.flush()

	print("---------Download Comleted----------")

def merge(video_path, audio_path, output_path):
	print("正在合并文件...")
	try:
		# print( "%s" %(video_path, audio_path, output_path))
		os.system("ffmpeg -i  %s -i %s -c copy %s" %(video_path, audio_path, output_path))

		os.remove(video_path)
		os.remove(audio_path)

	except Exception as e:
		print("合并失败",e)

def get_apidata_list(cartoon_ss_num):
	global headers
	headers = {
		"Accept": "application/json, text/plain, */*",
		'cookie': "",
		'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num, 
		'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36',
	}

	# 23年方法:
	'https://api.bilibili.com/pgc/player/web/v2/playurl'
	# avid cid ep_id来源于:
	"""https://api.bilibili.com/pgc/view/web/season?ep_id=400973"""

	ep_id = cartoon_ss_num[2:]
	response = requests.get(url=f'https://api.bilibili.com/pgc/view/web/season?ep_id={ep_id}', headers=headers).text
	jsondata = json.loads(response)

	# pprint(jsondata)

	return jsondata['result']['episodes']

def get_apidata(title, cartoon_ss_num):
	lis = get_apidata_list(cartoon_ss_num)

	for ls in lis:
		aid = ls['aid']
		cid = ls['cid']
		ep_id = ls['ep_id']
		name = f"第{lis.index(ls)+1}话 - "+ls['long_title']
		if not os.path.isfile(title + os.sep + name + ".mp4"):
			link2 = "https://api.bilibili.com/x/player/playurl"
			link1 = 'https://api.bilibili.com/pgc/player/web/v2/playurl'
			data = {
				'support_multi_audio': 'true',
				'avid': aid,
				'cid': cid,
				'qn': '112',
				'fnver': '0',
				'fnval': '4048',
				'fourk': '1',
				'gaia_source': '',
				'from_client': 'BROWSER',
				'ep_id': ep_id,
				'session': '9cdebcb2725f4b85aecf0180ad66bf7c',
				'drm_tech_type': '2',
			}
			response_json = requests.get(url=link1, params=data, headers=headers, timeout=10000).json()

			# pprint(response_json)
			try :
				dush_date = response_json['result']['video_info']['dash']
			except:
				response_json = requests.get(url=link2, params=data, headers=headers, timeout=10000).json()
				dush_date = response_json['data']['durl']
				video_url = dush_date[0]['url']
				seps = ".mp4"
				fileDownload(url=video_url, name=title + os.sep + name, seps=seps)

			video_url = dush_date['video'][0]['baseUrl']
			seps = ".mp4"
			fileDownload(url=video_url, name=title + os.sep + name + "_", seps=seps)

			audio_url = dush_date['audio'][0]['baseUrl']
			seps = ".mp3"
			fileDownload(url=audio_url, name=title + os.sep + name + "_", seps=seps)
			merge(video_path=title + os.sep + name + "_.mp4", audio_path=title + os.sep + name + "_.mp3",
				  output_path=title + os.sep + name + ".mp4")
		else:
			pass
		# break


if __name__ == "__main__":
	cartoon_ss_num = ""

	headers = {
		'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num,
		'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36',
	}
	rsp = requests.get(url = "https://www.bilibili.com/bangumi/play/%s/"%cartoon_ss_num, headers=headers).text
	title = re.findall('<meta property="og:title" content="(.*?)"/>', rsp)[0].replace(' ','')
	if cartoon_ss_num[:2] == "ss":
		cartoon_ss_num = "ep" + re.findall('//www.bilibili.com/bangumi/play/ep(.*?)"/>', rsp)[0]
		pprint(cartoon_ss_num)
	if not os.path.exists(title):
		os.makedirs(title)

	get_apidata(title = title, cartoon_ss_num = cartoon_ss_num)
