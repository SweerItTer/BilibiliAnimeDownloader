import json
import os
import re

import requests

class BAD:

	@staticmethod
	def fileDownload(url, name, seps):

		print("-----------Downloading------------")

		# 发送option请求服务器分配资源
		session = requests.session()

		session.options(url=url, headers=headers, verify=False)

		res = session.get(url=url, headers=headers, verify=False, timeout = 50)


		with open(name+seps, 'wb') as fp:

			fp.write(res.content)

			fp.flush()

		print("---------Download Comleted----------")

	@staticmethod
	def merge(video_path, audio_path, output_path):
		print("正在合并文件...")
		try:
			# print( "%s" %(video_path, audio_path, output_path))
			os.system(f"{os.getcwd()}/ffmpeg -i  {video_path} -i {audio_path} -c copy {output_path}")

			os.remove(video_path)
			os.remove(audio_path)

		except Exception as e:
			print("合并失败",e)

	@staticmethod
	def get_apidata_list(cartoon_ss_num):
		global headers
		headers = {
			'cookie': "", # 引号这里放cookie
			'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num,  # 防盗链
			'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36',
		}

		# 23年方法:
		'https://api.bilibili.com/pgc/player/web/v2/playurl'

		# avid cid ep_id来源于:
		"""https://api.bilibili.com/pgc/view/web/season?ep_id=400973"""


		response = requests.get(url=f'https://api.bilibili.com/pgc/view/web/season?ep_id={cartoon_ss_num.split("p")[-1]}', headers=headers).text
		jsondata = json.loads(response)

		return jsondata['result']['episodes']

	def get_apidata(self, title, cartoon_ss_num):
		lis = self.get_apidata_list(cartoon_ss_num)

		for ls in lis:
			aid = ls['aid']
			cid = ls['cid']
			ep_id = ls['ep_id']
			name = ls['long_title']
			link = 'https://api.bilibili.com/pgc/player/web/v2/playurl'
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
			response_json = requests.get(url=link, params=data, headers=headers, timeout=10000).json()
			# dush_date = response_json['result']['video_info']['dash'] 24年好似改为了"durl"
			try :
				dush_date = response_json['result']['video_info']['dash']
			except KeyError:
				dush_date = response_json['result']['video_info']['durl']

			video_url = dush_date['video'][0]['baseUrl']
			seps = ".mp4"
			self.fileDownload(url = video_url, name = title+os.sep+name+"_", seps = seps)

			audio_url = dush_date['audio'][0]['baseUrl']
			seps = ".mp3"
			self.fileDownload(url = audio_url, name = title+os.sep+name+"_", seps = seps)

			self.merge(video_path = title+os.sep+name+"_.mp4", audio_path = title+os.sep+name+"_.mp3", output_path = title+os.sep+name+".mp4")
			# pprint(response_json)
			break


if __name__ == "__main__":
	cartoon_ss_num = "" # 引号这里放ep号
	headers = {
		'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num,
		'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36',
	}
	title = re.findall('<meta property="og:title" content="(.*?)"/>', requests.get(url = "https://www.bilibili.com/bangumi/play/%s/"%cartoon_ss_num, headers=headers).text)[0].replace(' ','')
	if not os.path.exists(title):
		os.makedirs(title)

	BAD().get_apidata(title = title, cartoon_ss_num = cartoon_ss_num)
