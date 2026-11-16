"""SP-404MKII project files: PADCONF.BIN and one SMPL/BANKb-pp.SMP per used pad.

Roland does not publish these formats. The layouts follow the SparkyMK2 notes
(github.com/TuscanPL/SparkyMK2, docs/re/02-files.md and 03-parameters.md), checked against
a project exported by an SP-404MKII on firmware 5.x. All integers are big-endian.
"""
import hashlib
import os
import re
import struct
import unicodedata

import numpy as np

PADS = 160
SR = 48000
SMP_HEADER = 512
PADCONF_LEN = 52000
RECORDS, NAMES, CHOPS = 0xA0, 0x6C20, 0x7B20
RECORD_WORDS, NAME_LEN, CHOP_WORDS = 43, 24, 32
PROJECT_NAME_LEN = 32

# Every empty pad in a device export holds this record. Field n is pad parameter 0x66 + n:
# level 127, gate on, field 6 set, BPM 90.00, time stretch 100%, balance centre, bus 1, roll 2,
# hold 100.
EMPTY_PAD = (0, 0, 0, 127, 1, 0, 1, 0, 0, 9000, 0, 0, 0, 0, 0, 0, 10000, 1, 64, 0, 1, 2, 0, 100) + (0,) * 19
SIZE, START, END, BPM, LOOP_TOP = 0, 1, 2, 9, 11

# The 28 project setting words of a device export. Word 0 is the project tempo, words 12 to 21
# hold each bank's tempo shifted left by one with bit 0 for protect. The meaning of the
# others is not known, so they are kept as the device wrote them.
SETTINGS = [9000, 0, 0, 64, 64, 64, 0, 1, 12, 2, 3, 13] + [9000 << 1] * 10 + [0, 1179669, 2555904, 1638415, 1769477, 13]


def ascii_text(s, n):
    s = unicodedata.normalize("NFKD", s).encode("ascii", "ignore").decode()
    return re.sub(r"[^ -~]", "_", s)[:n]


def bank_file(pad):
    return f"BANK{pad // 16 + 1}-{pad % 16 + 1:02d}.SMP"


def smp_bytes(y):
    """An SMP file from float samples shaped (channels, frames), channels 1 or 2, at 48 kHz."""
    # Scaled by 32768 like the float samples soundfile reads, so 16-bit sources come back exact.
    pcm = np.clip(np.round(np.asarray(y, np.float64) * 32768), -32768, 32767).astype(">i2").T.tobytes()
    fields = struct.pack(">IIII", len(pcm) + SMP_HEADER - 8, SR, y.shape[0], 16)
    head = b"RFWV" + fields + bytes(12) + hashlib.md5(fields).digest()
    return head + os.urandom(SMP_HEADER - len(head)) + pcm


def padconf_bytes(name, pads, tempo=90.0):
    """pads maps a pad number (0 = A1, 159 = J16) to (SMP file size, pad name, BPM or None)."""
    b = bytearray(PADCONF_LEN)
    b[0:16] = b"RFPD" + struct.pack(">I", RECORDS) + bytes([3, 0, 0, 0]) + struct.pack(">I", 0x7A80)
    settings = list(SETTINGS)
    settings[0] = int(round(tempo * 100))
    settings[12:22] = [settings[0] << 1] * 10
    b[0x10:0x80] = struct.pack(">28I", *settings)
    b[0x80:0x80 + PROJECT_NAME_LEN] = ascii_text(name, PROJECT_NAME_LEN).encode().ljust(PROJECT_NAME_LEN, b"\0")
    for pad in range(PADS):
        rec = list(EMPTY_PAD)
        label = b" " * (NAME_LEN - 1)
        if pad in pads:
            size, pname, bpm = pads[pad]
            rec[SIZE] = rec[END] = size
            rec[START] = rec[LOOP_TOP] = SMP_HEADER
            if bpm and 40 <= bpm <= 200:
                rec[BPM] = int(round(bpm * 100))
            label = ascii_text(pname, NAME_LEN - 1).encode().ljust(NAME_LEN - 1, b"\0")
        struct.pack_into(">43i", b, RECORDS + pad * RECORD_WORDS * 4, *rec)
        b[NAMES + pad * NAME_LEN: NAMES + (pad + 1) * NAME_LEN] = label + b"\0"
        struct.pack_into(">32i", b, CHOPS + pad * CHOP_WORDS * 4, *([-1] * 16 + [0] * 16))
    return bytes(b)


PICTURES = ("startup_1", "startup_2", "screen_saver_1", "screen_saver_2", "screen_saver_3", "screen_saver_4")


def blank_picture():
    """A 128 x 64, 1-bit BMP for the device screen, every pixel dark: 62 bytes of headers and
    palette, then 16 bytes per row."""
    head = b"BM" + struct.pack("<IIII", 1086, 0, 62, 40) + struct.pack("<iiHHIIiiII", 128, 64, 1, 1, 0, 0, 0, 0, 0, 0)
    return head + bytes([0, 0, 0, 0, 255, 255, 255, 0]) + bytes(1024)


def read_padconf(data):
    """Project name and, for each used pad, (SMP file size, pad name)."""
    if data[:4] != b"RFPD" or data[8] != 3 or len(data) < PADCONF_LEN:
        raise ValueError("Not a version 3 PADCONF.BIN")
    name = data[0x80:0x80 + PROJECT_NAME_LEN].split(b"\0")[0].decode("ascii", "replace").strip()
    pads = {}
    for pad in range(PADS):
        size = struct.unpack_from(">I", data, RECORDS + pad * RECORD_WORDS * 4)[0]
        if size:
            label = data[NAMES + pad * NAME_LEN: NAMES + (pad + 1) * NAME_LEN].split(b"\0")[0].decode("ascii", "replace")
            pads[pad] = (size, label)
    return name, pads


def check_smp(data):
    """(channels, frames) of an SMP file, after checking its header digest."""
    if data[:4] != b"RFWV":
        raise ValueError("Not an SMP file")
    fields = data[4:20]
    if hashlib.md5(fields).digest() != data[0x20:0x30]:
        raise ValueError("SMP header digest does not match")
    size, sr, ch, bits = struct.unpack(">IIII", fields)
    if size + 8 != len(data) or sr != SR or bits != 16 or ch not in (1, 2):
        raise ValueError("SMP header does not describe the file")
    return ch, (len(data) - SMP_HEADER) // (2 * ch)
