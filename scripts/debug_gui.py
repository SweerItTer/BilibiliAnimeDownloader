#!/usr/bin/env python3
"""Open the production Windows GUI with private storage.

--live-anonymous uses real service responses with memory-only accounts.

Build gui normally first. Generated responses, logs and receipts stay in target/.
This script supplies data, not a second UI or a successful transfer simulation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid

TARGET = "x86_64-pc-windows-msvc"
DEAD_PROXY = "http://127.0.0.1:0"


def fixture_data() -> dict:
    responses = []

    def add(path, query, body, delay=0):
        responses.append({
            "path": path, "query": {key: str(value) for key, value in query.items()},
            "body": body, "delay_ms": delay,
        })

    add("/x/frontend/finger/spi", {}, {"code": 0, "data": {
        "b_3": "synthetic-device-3", "b_4": "synthetic-device-4"}})
    season = {"code": 0, "result": {
        "season_id": 9001, "title": "离线交互检查：24 集番剧", "type": 1,
        "evaluate": "本地合成响应，仅验证正式界面与核心解析。最后一集仅有720P；第23集返回目录查询错误。",
        "episodes": [
            {"id": 9100 + n, "aid": 9200 + n, "cid": 9300 + n, "title": str(n),
             "long_title": f"离线第 {n:02d} 集：" + ("核对原始身份与长标题换行" if n in (1, 24) else "交互检查"),
             "duration": 1200000, "is_preview": False}
            for n in range(1, 25)
        ],
    }}
    add("/pgc/view/web/season", {"season_id": 9001}, season)
    for n in range(1, 25):
        add("/pgc/view/web/season", {"ep_id": 9100 + n}, season)

    def streams(qualities, envelope):
        return {"code": 0, envelope: {
            "quality": max(qualities), "accept_quality": [120, 80, 64],
            "accept_description": ["4K 超清", "1080P 高清", "720P 高清"],
            "support_formats": [
                {"quality": 120, "new_description": "4K 超清"},
                {"quality": 80, "new_description": "1080P 高清"},
                {"quality": 64, "new_description": "720P 高清"},
            ],
            "dash": {"duration": 1200, "video": [
                {"id": q, "codecs": "avc1.640028", "width": 1920 if q == 80 else 1280,
                 "height": 1080 if q == 80 else 720, "frameRate": "30/1",
                 "bandwidth": 2000000 if q == 80 else 1000000,
                 "baseUrl": f"https://v.bilivideo.com/offline/video-{q}.m4s"}
                for q in qualities
            ], "audio": [
                {"id": 30280, "codecs": "mp4a.40.2", "bandwidth": 128000,
                 "audio_sampling_rate": 48000, "channels": 2,
                 "baseUrl": "https://a.bilivideo.com/offline/audio.m4s"}
            ]},
        }}

    for n in range(1, 25):
        for quality in (127, 120, 80, 64):
            body = ({"code": -403, "message": "offline catalog denied"} if n == 23 else
                    streams([64] if n == 24 or quality == 64 else [80, 64], "result"))
            add("/pgc/player/web/v2/playurl",
                {"ep_id": 9100 + n, "qn": quality, "fnval": 12240, "fourk": 1},
                body, 1600 if n == 24 and quality == 127 else 0)
    for aid, count in ((9401, 3), (9402, 1)):
        pages = [
            {"cid": 9500 + n if aid == 9401 else 9601, "page": n,
             "part": f"第 {n} 部分：原始分P选择", "duration": 60}
            for n in range(1, count + 1)
        ]
        add("/x/web-interface/view", {"aid": aid}, {"code": 0, "data": {
            "aid": aid, "title": "离线交互检查：" + ("分P视频" if count == 3 else "单个视频"),
            "desc": "本地合成数据，不连接线上服务。", "owner": {"name": "离线样例"},
            "duration": 180, "pages": pages,
        }})
        for page in pages:
            for quality in (127, 120, 80, 64):
                add("/x/player/wbi/playurl",
                    {"avid": aid, "cid": page["cid"], "qn": quality,
                     "fnver": 0, "fnval": 4048, "fourk": 1},
                    streams([64] if quality == 64 else [80, 64], "data"))
    # All visible PGC sections and their genre controls have local data.
    film = {"code": 0, "result": {
        "season_id": 9002, "title": "离线影视样例", "type": 2,
        "episodes": [{"id": 9201, "aid": 9301, "cid": 9401,
                      "title": "正片", "long_title": "离线影视样例", "duration": 1200000}],
    }}
    add("/pgc/view/web/season", {"season_id": 9002}, film)
    add("/pgc/view/web/season", {"ep_id": 9201}, film)
    for quality in (127, 120, 80, 64):
        add("/pgc/player/web/v2/playurl",
            {"ep_id": 9201, "qn": quality, "fnval": 12240, "fourk": 1},
            streams([64] if quality == 64 else [80, 64], "result"))
    for section in (1, 2, 3, 4, 5, 7):
        row = {"season_id": 9001 if section in (1, 4) else 9002,
               "title": "离线番剧样例" if section in (1, 4) else "离线影视样例"}
        add("/pgc/season/index/condition", {"season_type": section, "type": 1},
            {"code": 0, "data": {"filter": [{"field": "style_id", "name": "风格", "values": [
                {"keyword": "-1", "name": "全部"}, {"keyword": "1", "name": "演示类型"}]}]}})
        for genre in (-1, 1):
            add("/pgc/season/index/result", {"season_type": section, "st": section,
                "style_id": genre, "page": 1, "pagesize": 20},
                {"code": 0, "data": {"list": [row], "total": 1, "size": 20, "has_next": 0}})
    # A documented keyword exercises real signed search, sorting and paging.
    for kind in ("video", "media_bangumi", "media_ft"):
        for page in (1, 2):
            row = ({"aid": 9400 + page, "title": f"你好：离线视频 {page}"} if kind == "video" else
                   {"season_id": 9001 if kind == "media_bangumi" else 9002,
                    "title": "你好：离线番剧" if kind == "media_bangumi" else "你好：离线影视"})
            orders = (None, "click", "pubdate", "dm", "stow", "scores") if kind == "video" else (None,)
            channels = (None, 1, 3, 4, 5, 36, 119, 129, 155, 160, 181, 188, 202, 211, 217, 223, 234) if kind == "video" else (None,)
            for order in orders:
                for channel in channels:
                    query = {"search_type": kind, "keyword": "你好", "page": page}
                    if order is not None:
                        query["order"] = order
                    if channel is not None:
                        query["tids"] = channel
                    add("/x/web-interface/wbi/search/type", query, {"code": 0, "data": {
                        "result": [row], "page": page, "pagesize": 1, "numPages": 2, "numResults": 2}})
    add("/x/web-interface/nav", {}, {"code": 0, "data": {"isLogin": False, "wbi_img": {
        "img_url": "https://i0.hdslb.com/bfs/wbi/0123456789abcdef0123456789abcdef.png",
        "sub_url": "https://i0.hdslb.com/bfs/wbi/fedcba9876543210fedcba9876543210.png",
    }}})
    return {"schema_version": 1, "responses": responses}


def child_environment(repo: Path, host: Path, environ, live_anonymous=False) -> dict:
    env = {key: environ[key] for key in ("SystemRoot", "WINDIR", "SystemDrive") if key in environ}
    env.update({
        "PATH": os.pathsep.join((str(repo / "target/native" / TARGET / "bin"),
                                 str(Path(environ["SystemRoot"]) / "System32"))),
        "HOME": str(host / "home"), "USERPROFILE": str(host / "home"),
        "APPDATA": str(host / "home/AppData/Roaming"),
        "LOCALAPPDATA": str(host / "home/AppData/Local"),
        "TEMP": str(host / "tmp"), "TMP": str(host / "tmp"),
        "HTTP_PROXY": DEAD_PROXY, "HTTPS_PROXY": DEAD_PROXY, "ALL_PROXY": DEAD_PROXY,
        "NO_PROXY": "", "BILI_GUI_ISOLATED_DEBUG_DIR": str(host / "app"),
        "BILI_GUI_OFFLINE_REPLAY": str(host / "responses.json"),
    })
    if live_anonymous:
        for key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "BILI_GUI_OFFLINE_REPLAY", "BILI_GUI_ISOLATED_DEBUG_DIR"):
            env.pop(key, None)
        # Preserve normal transport configuration rather than a debug proxy.
        for key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "all_proxy", "no_proxy"):
            if key in environ:
                env[key] = environ[key]
        env["BILI_GUI_ANONYMOUS_LIVE_DIR"] = str(host / "app")
    return env


def main() -> int:
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=repo / "target/gui-debug")
    parser.add_argument("--generate-only", action="store_true")
    parser.add_argument("--live-anonymous", action="store_true",
                        help="Use real network responses, fresh private storage and memory-only accounts; no replay")
    parser.add_argument("--network-probe", type=Path,
                        help="Optional local-only transport probe executable; failure prevents launch")
    args = parser.parse_args()
    if args.live_anonymous and (args.generate_only or args.network_probe):
        parser.error("live anonymous launch cannot generate replay or run an offline proxy probe")
    work = args.directory.resolve()
    if not work.is_relative_to((repo / "target").resolve()):
        parser.error("generated debug output must stay under the repository target directory")
    host = work / "sessions" / uuid.uuid4().hex
    for part in ("home/AppData/Roaming", "home/AppData/Local", "tmp"):
        (host / part).mkdir(parents=True, exist_ok=False)
    if not args.live_anonymous:
        (host / "responses.json").write_text(json.dumps(fixture_data(), ensure_ascii=False), encoding="utf-8")
    if args.generate_only:
        print(host / "responses.json")
        return 0
    if os.name != "nt":
        parser.error("this visible GUI launch recipe requires native Windows")
    entry = repo / "target" / TARGET / "release/gui.exe"
    env = child_environment(repo, host, os.environ, args.live_anonymous)
    if args.network_probe:
        probe = subprocess.run([str(args.network_probe.resolve())], env=env, cwd=host,
                               capture_output=True, text=True, timeout=15,
                               creationflags=subprocess.CREATE_NO_WINDOW)
        (host / "proxy-check.log").write_text(probe.stdout + probe.stderr, encoding="utf-8")
        if probe.returncode:
            raise SystemExit("Network isolation probe failed; GUI was not launched")
    with (host / "stdout.log").open("wb") as out, (host / "stderr.log").open("wb") as err:
        process = subprocess.Popen([str(entry)], env=env, cwd=host, stdin=subprocess.DEVNULL,
                                   stdout=out, stderr=err, creationflags=subprocess.CREATE_NO_WINDOW)
    receipt = {"pid": process.pid, "entry": str(entry), "host": str(host), "root": str(host / "app"),
               "sha256": hashlib.sha256(entry.read_bytes()).hexdigest(),
               "ui": "production main/App/Bridge/Core",
               "network": "live service; no replay" if args.live_anonymous else "closed loopback proxy + Core replay",
               "accounts": "MemoryOnly", "scenario_inputs": ["你好"] if args.live_anonymous else ["你好", "ss9001", "ep9101", "av9401", "av9402"]}
    content = json.dumps(receipt, indent=2)
    (host / "launch.json").write_text(content, encoding="utf-8")
    (work / "current-launch.json").write_text(content, encoding="utf-8")
    print(content)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
