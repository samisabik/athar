import struct

import numpy as np
import pytest
import soundfile as sf

from athar import sp404


def test_smp_header_carries_the_digest_and_16_bit_audio_comes_back_exact():
    ints = np.array([[-32768, -1, 0, 1, 32767], [5, 6, 7, 8, 9]], np.int16)
    data = sp404.smp_bytes(ints / 32768)
    assert sp404.check_smp(data) == (2, 5)
    assert len(data) == 512 + ints.size * 2
    assert np.array_equal(np.frombuffer(data[512:], ">i2").reshape(-1, 2).T, ints)
    mono = sp404.smp_bytes(np.zeros((1, 10)))
    assert sp404.check_smp(mono) == (1, 10)
    with pytest.raises(ValueError):
        sp404.check_smp(data[:40] + b"x" + data[41:])
    with pytest.raises(ValueError):
        sp404.check_smp(b"RFWV" + data[4:0x20] + bytes(16) + data[0x30:])


def test_padconf_points_used_pads_at_their_files_and_leaves_the_rest_empty():
    data = sp404.padconf_bytes("Dusty Loops", {0: (1000, "kick", None), 17: (2048, "a_rather_long_loop_name_here", 92.5)})
    assert len(data) == sp404.PADCONF_LEN
    name, pads = sp404.read_padconf(data)
    assert name == "Dusty Loops"
    assert pads == {0: (1000, "kick"), 17: (2048, "a_rather_long_loop_name")}
    rec = lambda n: struct.unpack_from(">43i", data, sp404.RECORDS + n * 172)
    assert rec(17)[:3] == (2048, 512, 2048) and rec(17)[9] == 9250 and rec(17)[11] == 512
    assert rec(0)[9] == 9000
    assert rec(5) == sp404.EMPTY_PAD
    assert data[sp404.NAMES + 5 * 24: sp404.NAMES + 6 * 24] == b" " * 23 + b"\0"


def test_export_writes_a_project_folder_with_the_pad_layout(lib, add, tmp_path):
    pid = lib.create_project("kit")["id"]
    a, b, c = (add(n, seed=i, project=pid) for i, n in enumerate(("a.wav", "b.wav", "c.wav")))
    lib.set_project_pads(pid, {b: 0, a: 20})
    card = tmp_path / "card"
    (card / "IMPORT").mkdir(parents=True)
    out = lib.export_project(pid, str(card), fmt="sp404")
    folder = card / "IMPORT" / "kit"
    assert out["folder"] == str(folder)
    assert sorted(p.name for p in (folder / "SMPL").iterdir()) == ["BANK1-01.SMP", "BANK1-02.SMP", "BANK2-05.SMP"]
    assert sorted(p.name for p in folder.iterdir()) == ["PADCONF.BIN", "PICTURE", "PTN", "SMPL"]
    pic = (folder / "PICTURE" / "startup_1.bmp").read_bytes()
    assert len(pic) == 1086 and pic[:2] == b"BM" and struct.unpack_from("<ii", pic, 18) == (128, 64)
    name, pads = sp404.read_padconf((folder / "PADCONF.BIN").read_bytes())
    assert name == "kit"
    assert {n: label for n, (_, label) in pads.items()} == {0: "b", 1: "c", 20: "a"}
    smp = (folder / "SMPL" / "BANK1-01.SMP").read_bytes()
    assert pads[0][0] == len(smp)
    ch, frames = sp404.check_smp(smp)
    y, sr = sf.read(b, dtype="int16", always_2d=True)
    assert (ch, frames, sr) == (y.shape[1], y.shape[0], 48000)
    assert np.array_equal(np.frombuffer(smp[512:], ">i2").reshape(-1, ch), y)


def test_export_refuses_more_sounds_than_pads(lib, add, tmp_path, monkeypatch):
    pid = lib.create_project("big")["id"]
    add("a.wav", project=pid)
    monkeypatch.setattr("athar.library.SP404_PADS", 0)
    with pytest.raises(ValueError):
        lib.export_project(pid, str(tmp_path), fmt="sp404")


def test_export_keeps_another_project_with_the_same_folder_name(lib, add, tmp_path):
    pid = lib.create_project("kit")["id"]
    add("a.wav", project=pid)
    (tmp_path / "IMPORT" / "kit").mkdir(parents=True)
    (tmp_path / "IMPORT" / "kit" / "PADCONF.BIN").write_bytes(sp404.padconf_bytes("other", {}))
    with pytest.raises(ValueError):
        lib.export_project(pid, str(tmp_path), fmt="sp404")
    (tmp_path / "IMPORT" / "kit" / "PADCONF.BIN").write_bytes(sp404.padconf_bytes("kit", {}))
    lib.export_project(pid, str(tmp_path), fmt="sp404")
    assert (tmp_path / "IMPORT" / "kit" / "SMPL" / "BANK1-01.SMP").is_file()


def test_the_card_is_the_volume_with_import_and_export(tmp_path):
    from athar.library import sp404_card
    assert sp404_card(str(tmp_path)) is None
    (tmp_path / "Backup" / "Photos").mkdir(parents=True)
    for v in ("CARD", "SP404MKII"):
        (tmp_path / v / "IMPORT").mkdir(parents=True)
        (tmp_path / v / "EXPORT").mkdir()
    assert sp404_card(str(tmp_path)) == str(tmp_path / "SP404MKII")
