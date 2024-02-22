# 项目(脚本)功能
根据ep下载番剧全集
*  不允许用于商业

# 设置参数(其实是修改变量)
其中cookie要手动更新  
(后续有可能会更新可视化界面,登入b站以获取cookie)  
ep号也需要手动设置(后续可能会更新)  

在源码的这个位置更新自己的cookie  
```
def get_apidata_list(cartoon_ss_num):
	global headers
	headers = {
		'cookie': cookie,
		'referer': 'https://www.bilibili.com/bangumi/play/%s/' % cartoon_ss_num,  # 防盗链',
		'user-agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36',
	}
```
 这个位置更新ep号  
```
 if __name__ == "__main__":
	cartoon_ss_num = ep_id
```
# 使用说明

脚本使用非常简单,下载源文件,根据报错配置第三方库  
```pip install ****```  
zip压缩包是ffmpeg,合并音视频的工具,记得解压   
# 一些想说的话
如果要优化或者拿去它用,给个建议,因为清晰度请求数据保存在cookie里,要修改太麻烦了,所有有这么一个发现:  
调整视频的清晰度,视频url里的某一部分会发生变化   
```
比如:
清晰度从"1080p 高码率"切换到"1080p 高清"时，
330303894_nb2-1-30112这一部分部分的确变为了330303894_nb2-1-30080
```
对照json数据清晰度的对应关系如下  
```
{...,
accept_description: ["超清 4K", "高清 1080P+", "高清 1080P", "高清 720P", "清晰 480P", "流畅 360P"],
accept_format: "hdflv2,hdflv2,flv,flv720,flv480,mp4",
accept_quality: [120, 112, 80, 64, 32, 16],
...,}
```
不难发现  
30112 变成 30080  
其中122变成80,这或许意味着可以手动更换清晰度(没试过,猜的)  

# 再次声明
* 本脚本允许改编优化,但是所有使用者的运行结果以及其用途与本人无关
* 顺便提一下,只有开通了大会员的cookie才可以下付费内容,所以说如果想白嫖,可以略过该项目了
