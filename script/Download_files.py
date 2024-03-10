import os

def merge(video_path, audio_path, output_path):
	print("正在合并文件...")
	try:
		os.system("ffmpeg -i  %s -i %s -c copy %s" %(video_path, audio_path, output_path))
		if os.path.isfile(output_path):
			os.remove(video_path)
			os.remove(audio_path)
		else:
			print("合并失败")
	except Exception as e:
		print("合并失败",e)

def get_apidata(title, file_name, ids_d, cartoon_ss_num, sessions):

	cid = ids_d["cid"]
	aid = ids_d["aid"]
	ep_id = ids_d["ep_id"]

	# 判断是否存在已下载文件
	if os.path.isfile(title + os.sep + file_name + ".mp4"):
		return

	link2 = "https://api.bilibili.com/x/player/playurl"
	link1 = 'https://api.bilibili.com/pgc/player/web/v2/playurl'

	global headers
	headers = {
		"Accept": "application/json, text/plain, */*",
		'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num,  # 什么?B站防盗链?就这',
		'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36',
	}
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
	sessions.headers.update(headers)

	response_json = sessions.get(url=link1, params=data, timeout=10000).json()

	# pprint(response_json)
	try :
		dush_date = response_json['result']['video_info']['dash']
		# 视频
		video_url = dush_date['video'][0]['baseUrl']
		# 音频
		audio_url = dush_date['audio'][0]['baseUrl']
	except:
		response_json = sessions.get(url=link2, params=data, headers=headers, timeout=10000).json()
		dush_date = response_json['data']['durl']
		video_url = dush_date[0]['url']
		audio_url = ""

		# seps = ".mp4"
		# fileDownload(url=video_url, name=title + os.sep + name, seps=seps)

	# print(f"AudioUrl:{audio_url}")
	# print(f"VideoUrl:{video_url}")

	return {
		"AudioUrl": audio_url,
		"VideoUrl": video_url}

	# seps = ".mp4"
	# fileDownload(url=video_url, name=title + os.sep + name + "_", seps=seps)

	# seps = ".mp3"
	# fileDownload(url=audio_url, name=title + os.sep + name + "_", seps=seps)

	# # 合并
	# merge(video_path=title + os.sep + name + "_.mp4", audio_path=title + os.sep + name + "_.mp3",
	# 	  output_path=title + os.sep + name + ".mp4")
