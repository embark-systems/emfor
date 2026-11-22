#!/usr/bin/env python3
"""Write the SVG pictures of embark.systems: the cover of a release post
(emfor-<version>[.zh].svg, 1200x675) and the architecture diagram of the
manual (emfor-architecture[.zh].svg).

usage: tools/site/gen-diagrams.py OUT_DIR
PNGs of the covers are screenshots of the SVGs, for example with Chrome:
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless \\
    --hide-scrollbars --screenshot=out.png --window-size=1200,675 file://$PWD/emfor-0.5.0.svg
The cover below is the one of 0.5.0; copy cover() for the next release.
"""
import math
import os
import sys

out = sys.argv[1]
HEAD = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="%d" height="%d" viewBox="0 0 %d %d" '
    "font-family=\"-apple-system, 'Helvetica Neue', Arial, 'PingFang TC', 'Noto Sans TC', sans-serif\">\n"
    '<rect width="%d" height="%d" fill="#ffffff"/>\n'
)
MONO = "Menlo, Consolas, monospace"


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def text(x, y, s, size=16, w=400, fill="#1f2937", anchor="start", mono=False):
    f = f' font-family="{MONO}"' if mono else ""
    return f'<text x="{x}" y="{y}" font-size="{size}" font-weight="{w}" fill="{fill}" text-anchor="{anchor}"{f}>{esc(s)}</text>\n'


def box(x, y, w, h, fill, stroke, sw=2, rx=12):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"/>\n'


def arrow(x1, y1, x2, y2, color="#5b6472", sw=2, dash=None):
    a = math.atan2(y2 - y1, x2 - x1)
    L = 11
    W = 6
    bx, by = x2 - L * math.cos(a), y2 - L * math.sin(a)
    d = f' stroke-dasharray="{dash}"' if dash else ""
    p = f"{x2},{y2} {bx-W*math.sin(a)},{by+W*math.cos(a)} {bx+W*math.sin(a)},{by-W*math.cos(a)}"
    return f'<line x1="{x1}" y1="{y1}" x2="{bx}" y2="{by}" stroke="{color}" stroke-width="{sw}"{d}/>\n<polygon points="{p}" fill="{color}"/>\n'


def cover(lang):
    T = {
        "en": dict(
            title="emfor 0.5.0: a fork that could hang for ever on FreeBSD",
            sub="The hang is in FreeBSD's libthr, not in emfor; 0.4.0 has it too, and 0.5.0 works around it",
            parent="Parent process", t1="thread 1: a lookup", t1a="inside LMDB's process-shared mutex",
            t1b="holds libthr's pshared_lock for reading", t2="thread 2: fork()", t2a="libthr takes pshared_lock for reading only,",
            t2b="so fork does not wait for thread 1", fork="fork", child="Child process",
            c1="only thread 2 exists in the child", c2="thread 1's read hold is still there", c2a="and nothing will ever release it",
            c3="first lookup: needs the write side", c3a="→ waits for ever", fix="The fix in 0.5.0",
            fix1="On FreeBSD, fork waits until no lookup is inside the store (pthread_atfork handlers).",
            fix2="0.4.0: the fork test failed 10 of 10 runs. 0.5.0: 0 hangs, lookup throughput unchanged."),
        "zh": dict(
            title="emfor 0.5.0：FreeBSD 上 fork 後可能永遠卡住的問題",
            sub="卡死的原因在 FreeBSD 的 libthr，不在 emfor；0.4.0 也有，0.5.0 加了繞過的方法",
            parent="父行程", t1="執行緒 1：正在查詢", t1a="在 LMDB 的 process-shared mutex 裡",
            t1b="以讀取方式持有 libthr 的 pshared_lock", t2="執行緒 2：呼叫 fork()", t2a="libthr 的 fork 只以讀取方式取這把鎖，",
            t2b="所以不會等執行緒 1", fork="fork", child="子行程",
            c1="子行程裡只剩執行緒 2", c2="執行緒 1 的讀取持有還留著", c2a="而且不會有人釋放它",
            c3="第一次查詢：要取寫入端", c3a="→ 永遠等下去", fix="0.5.0 的修正",
            fix1="在 FreeBSD 上，fork 會等到沒有查詢在 store 裡（pthread_atfork handler）。",
            fix2="0.4.0：fork 測試 10 次失敗 10 次。0.5.0：0 次卡住，查詢吞吐量不變。"),
    }[lang]
    s = HEAD % (1200, 675, 1200, 675, 1200, 675)
    s += text(60, 66, T["title"], 34, 700)
    s += text(60, 102, T["sub"], 19, 400, "#5b6472")
    s += box(60, 140, 500, 330, "#f8fafc", "#9aa1ab")
    s += text(80, 172, T["parent"], 18, 700)
    s += box(85, 192, 450, 120, "#eaf1ff", "#2f6fde", 2.5)
    s += text(105, 224, T["t1"], 18, 700)
    s += text(105, 254, T["t1a"], 16, 400, "#5b6472")
    s += text(105, 282, T["t1b"], 16, 600, "#2f6fde")
    s += box(85, 332, 450, 120, "#ffffff", "#9aa1ab", 1.5)
    s += text(105, 364, T["t2"], 18, 700)
    s += text(105, 394, T["t2a"], 16, 400, "#5b6472")
    s += text(105, 422, T["t2b"], 16, 600, "#1f2937")
    s += arrow(560, 305, 640, 305, "#5b6472", 3)
    s += text(600, 292, T["fork"], 17, 700, "#5b6472", "middle", True)
    s += box(640, 140, 500, 330, "#fdecec", "#d64545", 2.5)
    s += text(660, 172, T["child"], 18, 700)
    s += box(665, 192, 450, 64, "#ffffff", "#9aa1ab", 1.5)
    s += text(685, 230, T["c1"], 17, 600)
    s += box(665, 272, 450, 76, "#ffffff", "#d64545", 2)
    s += text(685, 302, T["c2"], 17, 600, "#b83232")
    s += text(685, 330, T["c2a"], 16, 400, "#5b6472")
    s += box(665, 364, 450, 86, "#ffffff", "#d64545", 2)
    s += text(685, 394, T["c3"], 17, 600)
    s += text(685, 430, T["c3a"], 19, 700, "#b83232")
    s += box(60, 500, 1080, 139, "#fff4e0", "#e0a43a")
    s += text(84, 536, T["fix"], 18, 700)
    s += text(84, 578, T["fix1"], 18, 600)
    s += text(84, 614, T["fix2"], 16, 400, "#5b6472")
    return s + "</svg>\n"


def cover_060(lang):
    T = {
        "en": dict(
            title="emfor 0.6.0: take a backend out for a deploy, then put it back",
            sub="drain = true removes it from every choice; its checks keep running",
            cfg="conf.d/20-api.toml", reload="reload", emfor="emfor", drained="drained (checks: healthy)",
            healthy="healthy", client="What a client gets", lookup="lookup", lookup_a="10.0.0.12 or 10.0.0.13",
            dns="DNS A, AAAA, SRV", dns_a="only .12 and .13", alld="every backend drained", alld_a="ALL_DOWN / SERVFAIL",
            deploy="A rolling deploy with examples/deploy/emfor-drain.sh",
            steps=["drain on", "wait-idle", "stop, deploy, start", "wait-ready", "drain off"],
            note="A request in flight finishes after a drain; after a plain stop it is cut (tested with nginx 1.30)."),
        "zh": dict(
            title="emfor 0.6.0：部署時把 backend 拿出來，再放回去",
            sub="drain = true 讓它從所有選擇中消失；它的檢查照常進行",
            cfg="conf.d/20-api.toml", reload="reload", emfor="emfor", drained="drained (checks: healthy)",
            healthy="healthy", client="client 拿到的", lookup="lookup", lookup_a="10.0.0.12 或 10.0.0.13",
            dns="DNS 的 A、AAAA、SRV", dns_a="只有 .12 與 .13", alld="全部 backend 被 drain", alld_a="ALL_DOWN / SERVFAIL",
            deploy="用 examples/deploy/emfor-drain.sh 滾動部署",
            steps=["drain on", "wait-idle", "停止、部署、啟動", "wait-ready", "drain off"],
            note="drain 之後進行中的請求會做完；直接停掉則會被切斷（以 nginx 1.30 測過）。"),
    }[lang]
    s = HEAD % (1200, 675, 1200, 675, 1200, 675)
    s += text(60, 66, T["title"], 34, 700)
    s += text(60, 102, T["sub"], 19, 400, "#5b6472")
    # the config file
    s += box(60, 140, 420, 330, "#f8fafc", "#9aa1ab")
    s += text(80, 172, T["cfg"], 18, 700, "#1f2937", "start", True)
    s += text(80, 224, '{ addr = "10.0.0.11:8080", drain = true },', 14, 700, "#2f6fde", "start", True)
    s += text(80, 262, '{ addr = "10.0.0.12:8080" },', 14, 400, "#1f2937", "start", True)
    s += text(80, 300, '{ addr = "10.0.0.13:8080" },', 14, 400, "#1f2937", "start", True)
    s += arrow(480, 305, 540, 305, "#5b6472", 3)
    s += text(510, 292, T["reload"], 13, 700, "#5b6472", "middle", True)
    # emfor's state of the three backends
    s += box(540, 140, 290, 330, "#eaf1ff", "#2f6fde", 2.5)
    s += text(560, 172, T["emfor"], 18, 700)
    rows = [(190, "10.0.0.11:8080", T["drained"], "#f3f4f6", "#9aa1ab", "#5b6472", "6 4"),
            (282, "10.0.0.12:8080", T["healthy"], "#e8f6ed", "#1f7a3d", "#1f7a3d", None),
            (374, "10.0.0.13:8080", T["healthy"], "#e8f6ed", "#1f7a3d", "#1f7a3d", None)]
    for y, addr, state, fill, stroke, col, dash in rows:
        d = f' stroke-dasharray="{dash}"' if dash else ""
        s += (f'<rect x="560" y="{y}" width="250" height="80" rx="12" fill="{fill}" stroke="{stroke}" '
              f'stroke-width="2"{d}/>\n')
        s += text(576, y + 33, addr, 16, 700, "#1f2937", "start", True)
        s += text(576, y + 61, state, 15, 600, col)
    s += arrow(830, 305, 880, 305, "#5b6472", 3)
    # what the clients get
    s += box(880, 140, 260, 330, "#ffffff", "#9aa1ab")
    s += text(900, 172, T["client"], 18, 700)
    items = [(190, T["lookup"], T["lookup_a"], "#1f7a3d", True), (282, T["dns"], T["dns_a"], "#1f7a3d", False),
             (374, T["alld"], T["alld_a"], "#b83232", False)]
    for y, head, val, col, mono in items:
        s += box(900, y, 220, 80, "#ffffff", col, 2)
        s += text(914, y + 33, head, 16, 700, "#1f2937", "start", mono)
        s += text(914, y + 61, val, 15, 600, col)
    # the deploy steps
    s += box(60, 500, 1080, 139, "#fff4e0", "#e0a43a")
    s += text(84, 536, T["deploy"], 18, 700)
    x = 84
    for i, step in enumerate(T["steps"]):
        s += box(x, 552, 184, 36, "#ffffff", "#e0a43a", 1.5, 8)
        s += text(x + 92, 576, step, 14, 600, "#1f2937", "middle", not any(ord(c) > 127 for c in step))
        if i < len(T["steps"]) - 1:
            s += arrow(x + 184, 570, x + 208, 570, "#5b6472", 2)
        x += 208
    s += text(84, 620, T["note"], 16, 400, "#5b6472")
    return s + "</svg>\n"


def cover_071(lang):
    T = {
        "en": dict(
            title="emfor 0.7.1: the same daemon, a new manual, a test for every scenario",
            sub="No production code differs from 0.7.0; the packages, the documents and the tests do",
            c1="Unchanged since 0.7.0", c1r=["daemon", "store format 2", "plugin protocol 1", "libemfor.so.0.7", "config format"],
            same="same",
            c2="New in 0.7.1", c2r=["rpm ships the manual and rules", "manual in English and 繁體中文", "Ansible, Puppet, Salt, Chef", "emfor-check-staged.sh", "emfor lookup is a stable interface"],
            c3="Tests", c3r=["every spec scenario has a test", "daemon tests also run as root", "usage error of an unknown --dns", "no metrics socket without [metrics]", "log line when a plugin stops answering"],
            up="Upgrading", up1="Nothing to change in your config.",
            up2="A daemon left at 0.7.0 keeps working with 0.7.1 clients and plugins."),
        "zh": dict(
            title="emfor 0.7.1：daemon 沒變，手冊是新的，每個情境都有測試",
            sub="正式程式碼和 0.7.0 一樣；動的是套件、文件和測試",
            c1="和 0.7.0 一樣", c1r=["daemon", "store 格式 2", "plugin 協定 1", "libemfor.so.0.7", "設定檔格式"],
            same="沒變",
            c2="0.7.1 新增", c2r=["rpm 附手冊與規則", "手冊有英文和繁體中文", "Ansible、Puppet、Salt、Chef", "emfor-check-staged.sh", "emfor lookup 算穩定介面"],
            c3="測試", c3r=["spec 每個情境都有測試", "daemon 測試也用 root 跑", "未知 --dns 會得到用法錯誤", "沒有 [metrics] 就不開 socket", "plugin 不回應時會記 log"],
            up="怎麼升級", up1="設定檔不用改。",
            up2="daemon 留在 0.7.0，也能和 0.7.1 的 client 與 plugin 一起用。"),
    }[lang]
    s = HEAD % (1200, 675, 1200, 675, 1200, 675)
    s += text(60, 66, T["title"], 30, 700)
    s += text(60, 102, T["sub"], 19, 400, "#5b6472")
    cols = [
        (60, T["c1"], T["c1r"], "#f8fafc", "#9aa1ab", "#e8f6ed", "#1f7a3d"),
        (430, T["c2"], T["c2r"], "#eaf1ff", "#2f6fde", "#ffffff", "#2f6fde"),
        (800, T["c3"], T["c3r"], "#f8fafc", "#9aa1ab", "#ffffff", "#9aa1ab"),
    ]
    for i, (x, head, rows, bg, bd, rf, rs) in enumerate(cols):
        s += box(x, 140, 340, 330, bg, bd, 2.5 if i == 1 else 2)
        s += text(x + 20, 172, head, 18, 700)
        for j, r in enumerate(rows):
            y = 190 + 56 * j
            s += box(x + 20, y, 300, 48, rf, rs, 2)
            if i == 0:
                s += text(x + 36, y + 30, r, 15, 700, mono=True)
                s += text(x + 304, y + 30, T["same"], 15, 600, "#1f7a3d", "end")
            else:
                s += text(x + 36, y + 30, r, 15, 600)
    s += box(60, 500, 1080, 139, "#fff4e0", "#e0a43a")
    s += text(84, 536, T["up"], 18, 700)
    s += text(84, 578, T["up1"], 18, 600)
    s += text(84, 614, T["up2"], 16, 400, "#5b6472")
    return s + "</svg>\n"


def arch(lang):
    T = {
        "en": dict(title="How emfor fits on one host",
                   backends="backends", checks="checks over the network", daemon="emfor daemon", daemon2="one per host, no central server",
                   dns="DNS on 127.0.0.1", dns2="for programs without a client library;", dns3="answered from the daemon's memory",
                   store="local LMDB state store", store2="the daemon is its only writer", writes="writes", reads="reads",
                   client="client library", client2="Rust, C, Go, Python, Perl, PHP", status="emfor status", status2="also usable as a monitoring check",
                   note="Lookups read only the local store: no network, about a microsecond."),
        "zh": dict(title="emfor 在一台主機上的組成",
                   backends="backend", checks="透過網路檢查", daemon="emfor daemon", daemon2="每台主機一個，沒有中央伺服器",
                   dns="127.0.0.1 上的 DNS", dns2="給沒有 client 函式庫的程式；", dns3="從 daemon 的記憶體回答",
                   store="本機 LMDB state store", store2="daemon 是唯一的寫入者", writes="寫入", reads="讀取",
                   client="client 函式庫", client2="Rust、C、Go、Python、Perl、PHP", status="emfor status", status2="也可當成監控檢查",
                   note="查詢只讀本機的 store：不經網路，約一微秒。"),
    }[lang]
    W, H = 1100, 590
    s = HEAD % (W, H, W, H, W, H)
    s += text(30, 44, T["title"], 26, 700)
    s += box(30, 110, 200, 110, "#f8fafc", "#9aa1ab")
    s += text(130, 172, T["backends"], 19, 700, "#1f2937", "middle")
    s += box(400, 100, 300, 130, "#eaf1ff", "#2f6fde", 2.5)
    s += text(550, 160, T["daemon"], 21, 700, "#1f2937", "middle")
    s += text(550, 192, T["daemon2"], 14, 400, "#5b6472", "middle")
    s += arrow(400, 165, 232, 165, "#2f6fde", 3)
    s += text(312, 152, T["checks"], 13, 600, "#2f6fde", "middle")
    s += box(800, 100, 270, 130, "#ffffff", "#9aa1ab", 1.5)
    s += text(935, 148, T["dns"], 18, 700, "#1f2937", "middle", False)
    s += text(935, 176, T["dns2"], 13, 400, "#5b6472", "middle")
    s += text(935, 196, T["dns3"], 13, 400, "#5b6472", "middle")
    s += arrow(702, 165, 798, 165, "#5b6472", 2)
    s += box(380, 300, 340, 90, "#e8f6ed", "#1f7a3d", 2.5)
    s += text(550, 339, T["store"], 19, 700, "#1f2937", "middle")
    s += text(550, 367, T["store2"], 14, 400, "#5b6472", "middle")
    s += arrow(550, 232, 550, 298, "#1f7a3d", 3)
    s += text(566, 272, T["writes"], 14, 600, "#1f7a3d")
    s += box(180, 450, 360, 80, "#ffffff", "#9aa1ab", 1.5)
    s += text(360, 484, T["client"], 18, 700, "#1f2937", "middle")
    s += text(360, 510, T["client2"], 14, 400, "#5b6472", "middle")
    s += box(600, 450, 320, 80, "#ffffff", "#9aa1ab", 1.5)
    s += text(760, 484, T["status"], 18, 700, "#1f2937", "middle", True)
    s += text(760, 510, T["status2"], 14, 400, "#5b6472", "middle")
    s += arrow(470, 448, 520, 392, "#5b6472", 2)
    s += arrow(760, 448, 640, 392, "#5b6472", 2)
    s += text(584, 425, T["reads"], 14, 600, "#5b6472", "middle")
    s += text(30, 570, T["note"], 14, 400, "#5b6472")
    return s + "</svg>\n"


os.makedirs(out, exist_ok=True)
for lang, suf in (("en", ""), ("zh", ".zh")):
    open(f"{out}/emfor-0.5.0{suf}.svg", "w").write(cover(lang))
    open(f"{out}/emfor-0.6.0{suf}.svg", "w").write(cover_060(lang))
    open(f"{out}/emfor-0.7.1{suf}.svg", "w").write(cover_071(lang))
    open(f"{out}/emfor-architecture{suf}.svg", "w").write(arch(lang))
