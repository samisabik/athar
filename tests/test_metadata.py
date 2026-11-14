import os

from conftest import index


def meta(lib, path):
    c = lib.catalog_get(path)
    return c["tags"], c["rating"]


def test_atomic_replace_keeps_tags_and_project(lib, add):
    pid = lib.create_project("kit")["id"]
    path = add("kick.wav", tags=["keeper"], rating=5, project=pid)
    lib.move(path + ".tmp", path)
    assert meta(lib, path) == (["keeper"], 5)
    assert lib.project_paths(pid) == {path}
    assert lib.count() == 1


def test_rename_event_carries_metadata_over_existing_destination(lib, add):
    a = add("a.wav", seed=1, tags=["from-a"], rating=4)
    b = add("b.wav", seed=2, tags=["from-b"], rating=1)
    os.replace(a, b)
    lib.move(a, b)
    assert meta(lib, b) == (["from-a"], 4)
    assert lib.count() == 1


def test_folder_move_event_rekeys_everything(lib, add):
    pid = lib.create_project("kit")["id"]
    a = add("old/a.wav", tags=["x"], rating=3, project=pid)
    new = os.path.join(lib.folder, "new")
    os.rename(os.path.join(lib.folder, "old"), new)
    lib.move(os.path.join(lib.folder, "old"), new, is_dir=True)
    moved = os.path.join(new, "a.wav")
    assert meta(lib, moved) == (["x"], 3)
    assert lib.project_paths(pid) == {moved}
    assert lib.embedding_of(moved) is not None and lib.embedding_of(a) is None


def test_file_moved_while_closed_gets_its_tags_back(lib, add, monkeypatch):
    pid = lib.create_project("kit")["id"]
    old = add("kick.wav", tags=["keeper"], rating=5, project=pid)
    new = os.path.join(lib.folder, "drums", "kick 01.wav")
    os.mkdir(os.path.dirname(new))
    os.rename(old, new)
    monkeypatch.setattr(lib, "_index", lambda folder, paths: [index(lib, p) for p in paths])
    lib._scan(lib.folder)
    assert meta(lib, new) == (["keeper"], 5)
    assert lib.project_paths(pid) == {new}
    assert not lib.q("SELECT 1 FROM gone")


def test_deleted_file_keeps_tags_for_its_return(lib, add):
    path = add("rain.wav", tags=["kyoto"], rating=4)
    data = open(path, "rb").read()
    os.unlink(path)
    lib.forget(path)
    assert lib.count() == 0 and lib.all_tags() == []
    open(path, "wb").write(data)
    index(lib, path)
    assert meta(lib, path) == (["kyoto"], 4)
    assert lib.all_tags() == [{"tag": "kyoto", "count": 1}]


def test_search_hides_missing_file_without_dropping_tags(lib, add):
    path = add("rain.wav", tags=["kyoto"], rating=4)
    os.unlink(path)
    assert lib.search(sort="name") == []
    assert meta(lib, path) == (["kyoto"], 4)


def test_missing_folder_is_offline_and_kept(lib, add):
    path = add("rain.wav", tags=["kyoto"])
    size = os.path.getsize(path)
    os.rename(lib.folder, lib.folder + "-away")
    assert lib.folder_counts() == [{"path": lib.folder, "files": 1, "indexed": 1, "bytes": size, "offline": True}]
    assert lib.search(sort="name") == [] and lib.count() == 1
    os.rename(lib.folder + "-away", lib.folder)
    assert [r["path"] for r in lib.search(sort="name")] == [path]


def test_trash_purges(lib, add):
    pid = lib.create_project("kit")["id"]
    path = add("bad.wav", tags=["x"], project=pid)
    lib.forget(path, purge=True)
    assert meta(lib, path) == ([], 0) and lib.project_paths(pid) == set()


def test_catalog_cache_follows_writes(lib, add):
    path = add("rain.wav", tags=["one"])
    assert lib.search(sort="name", words="one")[0]["tags"] == ["one"]
    lib.catalog_set(path, {"tags": ["two"]})
    assert lib.search(sort="name", words="one") == []
    assert lib.search(sort="name", words="two")[0]["tags"] == ["two"]


def test_backup_written_once_per_day(lib, add, tmp_path):
    add("rain.wav", tags=["kyoto"])
    lib._backup()
    files = list((tmp_path / "backups").glob("catalog-*.json"))
    assert len(files) == 1 and "kyoto" in files[0].read_text()


def test_sp404_export_is_16_bit_48k_and_dithered(tmp_path):
    import numpy as np
    import soundfile as sf
    from athar.library import to_sp404
    src, out = tmp_path / "quiet.wav", tmp_path / "out.wav"
    t = np.arange(44100) / 44100
    sf.write(src, (0.4 / 32768 * np.sin(2 * np.pi * 440 * t)).astype(np.float32), 44100, subtype="FLOAT")
    to_sp404(src, out)
    info = sf.info(out)
    assert (info.samplerate, info.subtype) == (48000, "PCM_16")
    y, _ = sf.read(out, dtype="int16")
    assert np.any(y != 0), "a tone below one step is kept as dithered signal"

    exact = tmp_path / "exact.wav"
    sf.write(exact, (np.arange(48000) % 200 - 100).astype(np.int16), 48000, subtype="PCM_16")
    to_sp404(exact, out)
    assert np.array_equal(sf.read(out, dtype="int16")[0], sf.read(exact, dtype="int16")[0]), "already 16-bit 48 kHz: untouched"


def test_regions_are_kept_and_follow_a_move(lib, add):
    a = add("take.wav")
    saved = lib.regions_set(a, [{"start": 0.5, "end": 1.25, "name": "hit"}, {"start": 3, "end": 2}])
    assert saved == [{"start": 0.5, "end": 1.25, "name": "hit"}]
    b = os.path.join(lib.folder, "take 2.wav")
    os.rename(a, b)
    lib.move(a, b)
    assert lib.regions_get(b) == saved and lib.regions_get(a) == []
    lib.regions_set(b, [])
    assert lib.regions_get(b) == []


def test_saved_edit_inherits_metadata_once(lib, add):
    src = add("rain.wav", tags=["kyoto"], rating=4)
    dst = add("rain_edit.wav", seed=2)
    lib.catalog_copy(src, dst)
    assert meta(lib, dst) == (["kyoto"], 4)
    lib.catalog_set(dst, {"tags": ["mine"]})
    lib.catalog_copy(src, dst)
    assert meta(lib, dst)[0] == ["mine"]


def test_save_endpoint_writes_flac_and_mp3(tmp_path, monkeypatch):
    import io
    import numpy as np
    import soundfile as sf
    from fastapi.testclient import TestClient
    from athar import library as L, server
    monkeypatch.setattr(L.Library, "_boot", lambda self: None)
    monkeypatch.setattr(L.Library, "_worker", lambda self: None)
    monkeypatch.setattr(server, "lib", L.Library(tmp_path / "index.db"))
    buf = io.BytesIO()
    t = np.arange(48000) / 48000
    sf.write(buf, (0.3 * np.sin(2 * np.pi * 440 * t)).astype(np.float32), 48000, format="WAV", subtype="PCM_24")
    c = TestClient(server.app, base_url="http://127.0.0.1:8000")
    c.cookies.set("athar_8000", server.TOKEN)
    for fmt, kind in (("flac", "FLAC"), ("mp3", "MP3"), ("wav", "WAV")):
        r = c.post("/api/save", data={"dest_dir": str(tmp_path), "name": "tone.wav", "format": fmt},
                   files={"file": ("tone.wav", buf.getvalue(), "audio/wav")})
        assert r.status_code == 200, r.text
        info = sf.info(r.json()["path"])
        assert (info.format, info.samplerate) == (kind, 48000) and abs(info.duration - 1) < 0.06
    assert sf.info(str(tmp_path / "tone.flac")).subtype == "PCM_24"
    assert c.post("/api/save", data={"dest_dir": str(tmp_path), "name": "x", "format": "exe"},
                  files={"file": ("x", b"", "audio/wav")}).status_code == 400
