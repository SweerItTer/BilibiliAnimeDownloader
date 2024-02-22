# B-py-
根据ep下载番剧全集
*  不允许用于商业


其中cookie要手动更新  
(后续有可能会更新可视化界面,登入b站以获取cookie)  
ep号也需要手动设置(后续可能会更新)  

脚本使用非常简单,下载源文件,根据报错配置第三方库  
pip install ****


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
* 本脚本允许改编优化,但是所有使用者的运行结果以及其用途与本人无关
* 顺便提一下,只有开通了大会员的cookie才可以下付费内容,所以说如果想白嫖,可以略过该项目了
