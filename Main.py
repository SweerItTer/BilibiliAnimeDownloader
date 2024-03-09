import http.cookiejar
import os
import re
import sys

import requests
from PyQt5.QtCore import QThread, pyqtSignal, QObject
from PyQt5.QtCore import QTimer, Qt
from PyQt5.QtGui import QPixmap
from PyQt5.QtWidgets import (QVBoxLayout, QWidget, QListWidget, QApplication, QMainWindow, QMessageBox, \
                             QListWidgetItem, QAbstractItemView, QLabel, QProgressBar)

from Login import *
from interface import *
from script import Getinfo, CreatQR, Getanimelist, Download_files


class LoginWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.sessions = requests.session()
        self.cookies_path = './temp/cookies.txt'
        self.win = None
        self.data = None
        self.pixmap = None
        self.m_Position = None
        self.m_flag = None
        self.ui = Ui_MainWindow()
        self.ui.setupUi(self)

        try:
            self.data = CreatQR.get_qrurl()
            CreatQR.make_qrcode(self.data)
        except requests.exceptions.SSLError:
            print("网络连接错误")

        self.pixmap = QPixmap("./temp/Qr.png")
        self.ui.label_3.setPixmap(self.pixmap)
        self.ui.label_3.setScaledContents(True)
        self.ui.pushButton_6.setEnabled(False)  # 禁用按钮
        self.ui.pushButton_5.setEnabled(False)

        self.ui.pushButton_6.clicked.connect(self.update_qr)
        self.ui.pushButton_5.clicked.connect(self.interface)

        self.timer = QTimer(self)
        self.timer.timeout.connect(self.judgements)
        self.timer.start(1000)  # 设置间隔为1秒

        self.setWindowFlag(QtCore.Qt.FramelessWindowHint)
        self.setAttribute(QtCore.Qt.WA_TranslucentBackground)
        self.show()

    def judgements(self):
        """判断二维码是否失效"""
        try:
            code = CreatQR.judge(self.data)
        except TypeError:
            print("为获取到有效信息，网络连接错误")
            self.timer.stop()
            return
        if code == 0:
            self.ui.label_5.setText("    扫码成功   ")
            self.ui.pushButton_6.setEnabled(False)  # 禁用按钮
            self.ui.pushButton_5.setEnabled(True)
            self.timer.stop()
        elif code == 86038:
            self.ui.label_5.setText("  二维码已失效   ")
            self.ui.pushButton_6.setEnabled(True)  # 处理完成后重新启用按钮
        elif code == 86090:
            self.ui.label_5.setText("     未确定     ")
        else:
            self.ui.label_5.setText("     未扫码     ")

    def update_qr(self):
        self.data = CreatQR.get_qrurl()
        CreatQR.make_qrcode(self.data)
        self.pixmap = QPixmap("./temp/Qr.png")
        self.ui.label_3.setPixmap(self.pixmap)
        self.ui.label_3.setScaledContents(True)
        self.ui.pushButton_6.setEnabled(False)  # 禁用按钮

    def is_membership(self) -> bool:
        load_cookiejar = http.cookiejar.LWPCookieJar()
        load_cookiejar.load(self.cookies_path, ignore_discard=True, ignore_expires=True)
        load_cookies = requests.utils.dict_from_cookiejar(load_cookiejar)
        cookies = requests.utils.cookiejar_from_dict(load_cookies)
        headers = {
            'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) '
                          'Chrome/83.0.4103.116 Safari/537.36',
        }
        self.sessions.cookies = cookies
        # print(self.sessions.cookies)
        login_url = self.sessions.get("https://api.bilibili.com/x/web-interface/nav",
                                      verify=False,
                                      headers=headers).json()
        # pprint(login_url)
        if login_url['data']['vip']['label']['text'] == '':
            return False
        return True

    def keyPressEvent(self, event):
        # 在这里执行调试模式下的操作
        if event.modifiers() == (Qt.ControlModifier | Qt.ShiftModifier) and event.key() == Qt.Key_D and os.path.isfile(
                "./cookies.txt"):
            print("Developer mode activated")

            self.cookies_path = "./cookies.txt"
            self.ui.pushButton_5.setEnabled(True)
            self.ui.pushButton_5.click()
        # 传递事件给父类处理
        super().keyPressEvent(event)

    def mousePressEvent(self, event):
        if event.button() == QtCore.Qt.LeftButton and self.isMaximized() == False:
            self.m_flag = True
            self.m_Position = event.globalPos() - self.pos()  # 获取鼠标相对窗口的位置
            event.accept()
            self.setCursor(QtGui.QCursor(QtCore.Qt.OpenHandCursor))  # 更改鼠标图标

    def mouseMoveEvent(self, mouse_event):
        if QtCore.Qt.LeftButton and self.m_flag:
            self.move(mouse_event.globalPos() - self.m_Position)  # 更改窗口位置
            mouse_event.accept()

    def mouseReleaseEvent(self, mouse_event):
        self.m_flag = False
        self.setCursor(QtGui.QCursor(QtCore.Qt.ArrowCursor))

    def interface(self):
        global win
        print(self.is_membership())
        print(self.sessions)

        if self.is_membership():
            win = InterfaceWindow(is_member=True, sessions=self.sessions)
            # print(self.sessions)
        else:
            win = InterfaceWindow(is_member=False, sessions=self.sessions)
            # print(self.sessions)
        self.close()


class InterfaceWindow(QMainWindow, QObject):
    def __init__(self, is_member, sessions):
        super(InterfaceWindow, self).__init__()
        self.episodes = None
        self.title = None
        self.videopath_list = None
        self.audiopath_list = None
        self.outputpath_list = None
        self.merge = None
        self.anime_window = None
        self.download_thread_a = None
        self.download_thread_v = None
        self.url_dict = None
        self.cartoon_ss_num = None
        self.file_name = None
        self.sessions = sessions
        self.user_input_text = None
        self.is_member = is_member
        self.list_ = {}
        self.m_Position = None
        self.picture = None
        self.pixmap = None
        self.m_flag = None
        self.ui = Ui_MainWindow_inter()
        self.ui.setupUi(self)
        self.downloader = None
        self.complete = [0, 0, 0, 0]
        # 用于记录已完成的下载任务数和判断是否全部下载任务已完成
        # [0]记录所有下载任务的数量
        # [1]记录是否所有数据下载完成
        # [2]记录合并完成的数量
        # [3]记录是否全部合并完成
        self.should_merge_number = [0]
        self.list_widget = self.ui.listWidget
        # 将 listWidget 的选择模式设置为多选模式
        self.list_widget.setSelectionMode(QAbstractItemView.MultiSelection)

        if not self.is_member:
            self.show_info_dialog()

        self.ui.pushButton_2.clicked.connect(self.wake_downloader)
        self.ui.pushButton.clicked.connect(self.update_picture)
        # self.ui.pushButton_3.setEnabled(False)

        self.timer = QTimer(self)
        self.timer.timeout.connect(self.enable_push_button_3)
        self.timer.start(1000)  # 设置间隔为1秒

        self.timer = QTimer(self)
        self.timer.timeout.connect(self.merge_complete)
        self.timer.start(1000)  # 设置间隔为1秒

        self.ui.pushButton_3.clicked.connect(self.wake_merge)

        self.setWindowFlag(QtCore.Qt.FramelessWindowHint)
        self.setAttribute(QtCore.Qt.WA_TranslucentBackground)
        self.show()

    @QtCore.pyqtSlot()
    def enable_push_button_3(self):
        if self.complete[1] == 0:
            self.ui.pushButton_3.setEnabled(False)
        elif self.complete[1] == 1:
            self.ui.pushButton_3.setEnabled(True)

    def merge_complete(self):
        if self.complete[3] == 1:
            self.show_merge_dialog()
            self.timer.stop()

    @staticmethod
    def show_info_dialog():
        msg = QMessageBox()
        msg.setIcon(QMessageBox.Information)
        msg.setText("您不是大会员用户，下载可能受阻")
        msg.setWindowTitle("提示")
        msg.setStandardButtons(QMessageBox.Ok)
        msg.exec_()

    @staticmethod
    def show_merge_dialog():
        msg = QMessageBox()
        msg.setIcon(QMessageBox.Information)
        msg.setText("所有文件合并完成")
        msg.setWindowTitle("提示")
        msg.setStandardButtons(QMessageBox.Ok)
        msg.exec_()

    def update_picture(self):
        # print("dnm")
        self.user_input_text = self.ui.lineEdit.text()

        if len(self.user_input_text) > 10 and "?" in self.user_input_text:
            self.user_input_text = re.findall("/play/(.*?)\?spm_id", self.user_input_text)[0]

        if "ss" in self.user_input_text:
            self.user_input_text = Getanimelist.transmit_id("", self.user_input_text)

        # print(self.user_input_text)

        filepath, self.title = Getinfo.getinfo(self.user_input_text)
        if filepath is None or self.title is None:
            self.ui.label_2.clear()
            self.ui.label_3.setText("未找到番剧")
            self.list_widget.clear()
            self.list_.clear()
            print(self.user_input_text)
        else:
            self.picture = QPixmap(filepath)
            width = self.ui.label_2.width()
            height = self.ui.label_2.height()

            # 将原图片按比例缩放到 QLabel 的尺寸
            scaled_pixmap = self.picture.scaled(width, height, Qt.KeepAspectRatio, Qt.SmoothTransformation)

            # 设置 QLabel 的 QPixmap
            self.ui.label_2.setPixmap(scaled_pixmap)
            self.ui.label_3.setText(self.title)
            self.update_list()

    def update_list(self):
        self.list_widget.clear()
        self.list_.clear()

        if "ep" not in self.user_input_text:
            self.user_input_text = "ep" + self.user_input_text

        self.cartoon_ss_num = self.user_input_text
        self.cartoon_ss_num = Getanimelist.transmit_id(self.title, self.cartoon_ss_num)
        animelist = Getanimelist.get_apidata_list(self.cartoon_ss_num, self.sessions)

        for one in animelist:
            if self.is_member:
                if one["badge"] == "预告":
                    continue

                self.list_.update(
                    {one["share_copy"].split("》")[-1]:
                        {
                            "cid": one["cid"],
                            "aid": one["aid"],
                            "ep_id": one["ep_id"],
                        }
                    }
                )
            else:
                if one["badge"] == "预告" or one["badge"] == "会员":
                    continue

                self.list_.update(
                    {one["share_copy"].split("》")[-1]:
                        {
                            "cid": one["cid"],
                            "aid": one["aid"],
                            "ep_id": one["ep_id"],
                        }
                    }
                )

        for episode in self.list_.keys():
            # 创建一个QListWidgetItem对象，并设置其文本内容为当前标题
            item = QListWidgetItem(episode)
            # 将这个QListWidgetItem添加到QListWidget中
            self.list_widget.addItem(item)

    def wake_merge(self):
        if len(self.videopath_list) == len(self.list_widget.selectedItems()) and self.videopath_list is not None:
            self.merge = MergeThread(self.videopath_list, self.audiopath_list, self.outputpath_list, self.complete,
                                     self.should_merge_number)
            self.merge.start()
        else:
            return

    def wake_downloader(self):
        self.complete = [0, 0, 0, 0]

        if not len(self.list_widget.selectedItems()) >= 1:
            return
        selected_texts = [item.text() for item in self.list_widget.selectedItems()]
        # print("Selected Texts:", selected_texts)

        episodes = []  # 新增：用于存储选中的集数名称
        urls = []  # 新增：用于存储对应集数的URL字典

        self.videopath_list = []
        self.audiopath_list = []
        self.outputpath_list = []

        for selected in selected_texts:
            ids_d = self.list_[selected]
            file_name = selected.replace(" ", "-").replace("-|-", "-")
            self.url_dict = Download_files.get_apidata(
                title=self.title,
                file_name=file_name,
                ids_d=ids_d,
                cartoon_ss_num=self.cartoon_ss_num,
                sessions=self.sessions
            )
            self.videopath_list.append(f"./{self.title}/{file_name}_.mp4")
            self.audiopath_list.append(f"./{self.title}/{file_name}_.mp3")
            self.outputpath_list.append(f"./{self.title}/{file_name}.mp4")

            episodes.append(selected)
            urls.append(self.url_dict)

            """
            线程类实例化时变量是 self.thread_1 一定要加上self，如果不加，thread_1就是一个局部变量，
            当其所在方法运行结束的时候，它的生命周期也都结束了，但是这个线程里的程序很有可能还没有运行完！
            可能会报错：QThread ：Destroyed while thread is still running！
            """

        self.anime_window = AnimeWindow(sessions=self.sessions, episodes=episodes, url_dict_list=urls,
                                        title=self.title, complete=self.complete)  # 初始化 AnimeWindow 类
        self.anime_window.show()
        self.should_merge_number[0] = len(episodes)

    def keyPressEvent(self, event):
        if event.key() == QtCore.Qt.Key_Return or event.key() == QtCore.Qt.Key_Enter:
            # 如果按下的是回车键，触发按钮点击事件
            self.ui.pushButton.click()
        elif event.modifiers() & QtCore.Qt.ControlModifier:
            # 按住 Ctrl 键时，选择模式为多选
            self.list_widget.setSelectionMode(QAbstractItemView.MultiSelection)
        # 传递事件给父类处理
        super().keyPressEvent(event)

    def keyReleaseEvent(self, event):
        # 检测 Ctrl 键是否被释放
        if not (event.modifiers() & QtCore.Qt.ControlModifier):
            # 释放 Ctrl 键时，选择模式为单选
            self.list_widget.setSelectionMode(QAbstractItemView.SingleSelection)
        # 传递事件给父类处理
        super().keyReleaseEvent(event)

    def mousePressEvent(self, event):
        if event.button() == QtCore.Qt.LeftButton and self.isMaximized() == False:
            self.m_flag = True
            self.m_Position = event.globalPos() - self.pos()  # 获取鼠标相对窗口的位置
            event.accept()
            self.setCursor(QtGui.QCursor(QtCore.Qt.OpenHandCursor))  # 更改鼠标图标

    def mouseMoveEvent(self, mouse_event):
        if QtCore.Qt.LeftButton and self.m_flag:
            self.move(mouse_event.globalPos() - self.m_Position)  # 更改窗口位置
            mouse_event.accept()

    def mouseReleaseEvent(self, mouse_event):
        self.m_flag = False
        self.setCursor(QtGui.QCursor(QtCore.Qt.ArrowCursor))


class DownloadThread(QThread):
    progress_signal = pyqtSignal(float)

    def __init__(self, sessions, title, url, file_name, sep_, complete, episodes):
        super().__init__()
        self.sessions = sessions
        self.title = title
        self.url = url
        self.file_name = file_name
        self.sep_ = sep_
        self.episodes = episodes
        self.complete = complete

    def run(self):
        try:
            self.download_f()
        except Exception as e:
            print(f"{self.title} {self.file_name}下载失败")
            print("Error occurred during download:", e)
            # 发送进度为 -1，表示下载错误
            self.progress_signal.emit(-1)
        if self.complete[0] == len(self.episodes) * 2:
            print("---------All Downloaded----------")
            self.complete[1] = 1

    def download_f(self):
        print("-----------Downloading------------")
        file_path = self.title + os.sep + self.file_name + self.sep_

        res = self.sessions.get(url=self.url, verify=False, timeout=50, stream=True)

        total_size = int(res.headers.get('content-length', 0))
        downloaded_size = 0

        if os.path.isfile(file_path) and os.path.getsize(file_path) == total_size:
            print("---------file exist----------")
            self.progress_signal.emit(100)
            self.complete[0] = self.complete[0] + 1
            return

        if os.path.isfile(file_path) and os.path.getsize(file_path) <= total_size:
            self.continue_download(os.path.getsize(file_path), total_size)

            # print(total_size)
        else:
            with open(self.title + os.sep + self.file_name + self.sep_, 'wb') as fp:
                for chunk in res.iter_content(chunk_size=1024 * 1000):  # 逐块写入，每次写入10mb
                    if chunk:
                        fp.write(chunk)
                        downloaded_size += len(chunk)

                        # 计算当前下载进度百分比
                        if total_size > 0:
                            progress_percent = (downloaded_size / total_size) * 100
                        else:
                            progress_percent = 0

                        # 设置新的进度值
                        self.progress_signal.emit(progress_percent)

            print("---------Download Completed----------")
            self.complete[0] = self.complete[0] + 1

    def continue_download(self, downloaded_size, total_size):
        headers = {
            'Range': 'bytes={}-{}'.format(downloaded_size, total_size)}  # Requesting to download from the point where it left off
        res = self.sessions.get(url=self.url, headers=headers, verify=False, timeout=50, stream=True)

        with open(self.title + os.sep + self.file_name + self.sep_, 'ab') as fp:
            for chunk in res.iter_content(chunk_size=1024 * 1000):  # 逐块写入，每次写入10mb
                if chunk:
                    fp.write(chunk)
                    downloaded_size += len(chunk)
                    progress_percent = (downloaded_size / total_size) * 100
                    self.progress_signal.emit(progress_percent)

        print("---------Download Completed----------")
        self.complete[0] = self.complete[0] + 1


class MergeThread(QThread):
    def __init__(self, video_path, audio_path, output_path, complete, should_merge_number):
        super().__init__()
        self.video_path = video_path
        self.audio_path = audio_path
        self.output_path = output_path
        self.complete = complete
        self.should_merge_number = should_merge_number

    def run(self):
        print("-----------Merging------------")

        for path_v, path_a, path_o in zip(self.video_path, self.audio_path, self.output_path):
            while 1:
                print("正在检测文件是否存在...")
                if os.path.isfile(path_a) and os.path.isfile(path_a):
                    break
                continue
            print("正在合并文件...")
            os.system(
                f"ffmpeg -i  {os.path.realpath(path_v)} -i {os.path.realpath(path_a)} -c copy {os.path.realpath(path_o)}")
            if os.path.isfile(path_o) and os.path.isfile(path_a) and os.path.isfile(path_v):
                os.remove(path_a)
                os.remove(path_v)
                print("合并成功")
                self.complete[2] = self.complete[2] + 1
        if self.complete[2] == self.should_merge_number[0]:
            print("---------All Merged----------")
            self.complete[3] = 1
            return


class EpisodeItem(QWidget):
    def __init__(self, episode_name):
        super().__init__()

        self.episode_name = QLabel(episode_name)
        self.progress_bar = QProgressBar()
        self.progress_bar.setObjectName(episode_name)

        layout = QVBoxLayout()
        layout.addWidget(self.episode_name)
        layout.addWidget(self.progress_bar)
        self.setLayout(layout)

        self.download_thread = None

    def start_download(self):
        self.download_thread.progress_signal.connect(self.update_progress)  # 直接将信号连接到进度条的setValue槽
        self.download_thread.start()

    def update_progress(self, progress):
        self.progress_bar.setValue(int(progress))


class AnimeWindow(QWidget):
    def __init__(self, sessions, title, url_dict_list, episodes, complete):
        super().__init__()

        self.setWindowTitle("番剧下载管理器")
        self.url_dict_list = url_dict_list
        self.item_v = None
        self.item_a = None
        self.episodes = episodes
        self.complete = complete
        self.title = title
        self.sessions = sessions
        self.list_widget = QListWidget()

        assert len(self.episodes) == len(self.url_dict_list)  # 确保集数与URL数量一致

        for filename, url_dict in zip(self.episodes, self.url_dict_list):
            try:
                video_url, audio_url = url_dict["VideoUrl"], url_dict["AudioUrl"]
            except TypeError:
                print("请稍后再试,或重新登入")
                return
            self.item_v = EpisodeItem(filename.split("》")[-1] + " 视频")
            self.item_a = EpisodeItem(filename.split("》")[-1] + " 音频")

            listWidgetItem_v = QListWidgetItem()
            listWidgetItem_a = QListWidgetItem()
            listWidgetItem_v.setSizeHint(self.item_v.sizeHint())
            listWidgetItem_a.setSizeHint(self.item_a.sizeHint())
            self.list_widget.addItem(listWidgetItem_v)
            self.list_widget.addItem(listWidgetItem_a)
            self.list_widget.setItemWidget(listWidgetItem_v, self.item_v)
            self.list_widget.setItemWidget(listWidgetItem_a, self.item_a)
            # 创建并启动下载线程

            self.download_thread_v = DownloadThread(sessions=self.sessions,
                                                    title=self.title,
                                                    url=video_url,
                                                    file_name=filename.replace(" ", "-").replace("-|-", "-"),
                                                    sep_="_.mp4",
                                                    complete=self.complete,
                                                    episodes=self.episodes)  # 创建下载器对象
            self.download_thread_a = DownloadThread(sessions=self.sessions,
                                                    title=self.title,
                                                    url=audio_url,
                                                    file_name=filename.replace(" ", "-").replace("-|-", "-"),
                                                    sep_="_.mp3",
                                                    complete=self.complete,
                                                    episodes=self.episodes)  # 创建下载器对象

            self.item_v.download_thread = self.download_thread_v
            self.item_a.download_thread = self.download_thread_a
            self.item_v.start_download()
            self.item_a.start_download()

        vbox = QVBoxLayout()
        vbox.addWidget(self.list_widget)

        self.setLayout(vbox)
        self.setGeometry(300, 250, 350, 200)

        self.timer = QTimer(self)
        self.timer.timeout.connect(self.close_window)
        self.timer.start(1000)  # 设置间隔为1秒

    def close_window(self):
        if self.complete[1] == 1:
            self.timer.stop()
            self.close()
            return


if __name__ == "__main__":
    QtCore.QCoreApplication.setAttribute(QtCore.Qt.AA_EnableHighDpiScaling)
    app = QApplication(sys.argv)
    win = LoginWindow()
    sys.exit(app.exec_())
