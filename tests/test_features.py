import json
import os

import pytest

from athar import discover


def names(rows):
    return sorted(os.path.basename(r["path"]) for r in rows)


def test_subfolders_are_listed_with_counts(lib, add):
    add("top.wav")
    add("Hits/Kicks/k1.wav", seed=1)
    add("Hits/Snares/s1.wav", seed=2)
    add("Hits/Snares/s2.wav", seed=3)
    add("Loops/l1.wav", seed=4)
    top = lib.subfolders(lib.folder)
    assert [(s["name"], s["files"], s["nested"]) for s in top] == [("Hits", 3, True), ("Loops", 1, False)]
    assert [(s["name"], s["files"]) for s in lib.subfolders(os.path.join(lib.folder, "Hits"))] == [("Kicks", 1), ("Snares", 2)]
    assert lib.subfolders("/nowhere/at/all") == []


def test_search_narrows_to_a_subfolder_and_reports_it(lib, add):
    add("top.wav")
    add("Hits/Snares/s1.wav", seed=1)
    add("Hits/Kicks/k1.wav", seed=2)
    rows = lib.search(folder=lib.folder, under=os.path.join(lib.folder, "Hits"), sort="name")
    assert names(rows) == ["k1.wav", "s1.wav"]
    assert {r["sub"] for r in rows} == {"Hits/Kicks", "Hits/Snares"}
    assert lib.search(sort="name")[-1]["sub"] == ""


def test_words_match_the_subfolder_a_sound_sits_in(lib, add):
    add("Hits/Snares/one.wav", seed=1)
    add("Hits/Kicks/two.wav", seed=2)
    assert names(lib.search(words="snares")) == ["one.wav"]


def test_project_with_a_folder_copies_what_is_added(lib, add, tmp_path):
    src = add("kick.wav", tags=["keeper"], rating=4)
    kit = os.path.join(lib.folder, "kit")
    os.mkdir(kit)
    pid = lib.create_project("kit")["id"]
    lib.set_project_folder(pid, kit)
    copy = lib.project_add(pid, src)
    assert copy == os.path.join(kit, "kick.wav") and os.path.exists(copy) and os.path.exists(src)
    assert lib.catalog_get(copy)["tags"] == ["keeper"]
    # The same sound a second time reuses the copy.
    assert lib.project_add(pid, src) == copy
    assert sorted(os.listdir(kit)) == ["kick.wav"]
    assert lib.project_paths(pid) == {copy}


def test_setting_a_folder_moves_existing_members_into_it(lib, add):
    pid = lib.create_project("kit")["id"]
    a = add("a.wav", seed=1, project=pid)
    kit = os.path.join(lib.folder, "kit")
    os.mkdir(kit)
    lib.set_project_folder(pid, kit)
    assert lib.project_paths(pid) == {os.path.join(kit, "a.wav")} and os.path.exists(a)


def test_project_add_fails_cleanly_when_the_folder_is_gone(lib, add):
    src = add("kick.wav")
    kit = os.path.join(lib.folder, "kit")
    os.mkdir(kit)
    pid = lib.create_project("kit")["id"]
    lib.set_project_folder(pid, kit)
    os.rmdir(kit)
    with pytest.raises(ValueError):
        lib.project_add(pid, src)
    assert lib.project_paths(pid) == set()


def test_project_order_is_kept_and_new_sounds_go_last(lib, add):
    pid = lib.create_project("kit")["id"]
    a, b, c = (add(n, seed=i, project=pid) for i, n in enumerate(("a.wav", "b.wav", "c.wav")))
    assert lib.project_order(pid) == [a, b, c]
    lib.set_project_order(pid, [c, a, b])
    assert lib.project_order(pid) == [c, a, b]
    d = add("0-first-by-name.wav", seed=9, project=pid)
    assert lib.project_order(pid) == [c, a, b, d]


def test_export_follows_the_project_order(lib, add, tmp_path):
    pid = lib.create_project("kit")["id"]
    a, b = add("a.wav", seed=1, project=pid), add("b.wav", seed=2, project=pid)
    lib.set_project_order(pid, [b, a])
    out = lib.export_project(pid, str(tmp_path / "out"), "{n:03d}_{name}")
    assert [os.path.basename(f) for f in out["files"]] == ["001_b.wav", "002_a.wav"]


def test_backup_round_trip(lib, add, tmp_path):
    pid = lib.create_project("kit")["id"]
    a = add("a.wav", seed=1, tags=["keep"], rating=4, project=pid)
    b = add("b.wav", seed=2, project=pid)
    lib.catalog_set(a, {"memo": "under the bridge"})
    lib.set_project_order(pid, [b, a])
    file = lib.export_backup(str(tmp_path))
    assert json.load(open(file))["athar_backup"] == 1
    lib.catalog_set(a, {"tags": [], "rating": 0, "memo": ""})
    lib.delete_project(pid)
    assert lib.restore_backup(file) == {"sounds": 1, "projects": 1, "regions": 0}
    c = lib.catalog_get(a)
    assert (c["tags"], c["rating"], c["memo"]) == (["keep"], 4, "under the bridge")
    assert lib.project_order(lib.projects()[0]["id"]) == [b, a]


def test_restore_refuses_a_file_that_is_not_a_backup(lib, tmp_path):
    bad = tmp_path / "x.json"
    bad.write_text("[1, 2, 3]")
    with pytest.raises(ValueError):
        lib.restore_backup(str(bad))
    bad.write_text("not json")
    with pytest.raises(ValueError):
        lib.restore_backup(str(bad))


def test_sound_map_places_every_sound_inside_the_unit_square(lib, add):
    assert lib.sound_map() == []
    paths = [add(f"s{i}.wav", seed=i) for i in range(14)]
    points = lib.sound_map()
    assert sorted(p["path"] for p in points) == sorted(paths)
    assert all(0 <= p["x"] <= 1 and 0 <= p["y"] <= 1 for p in points)
    assert len({(p["x"], p["y"]) for p in points}) > 1
    assert lib.sound_map() is points


def test_folder_counts_include_size_on_disk(lib, add):
    a = add("a.wav")
    assert lib.folder_counts()[0]["bytes"] == os.path.getsize(a)


def test_only_the_archives_may_be_fetched():
    assert discover.allowed("https://api.openverse.org/v1/audio/")
    assert discover.allowed("https://ia800.us.archive.org/1/items/x/y.mp3")
    assert not discover.allowed("http://archive.org/x.mp3")
    assert not discover.allowed("https://archive.org.evil.example/x.mp3")
    assert not discover.allowed("https://evilarchive.org/x.mp3")
    assert not discover.allowed("file:///etc/passwd")
    with pytest.raises(discover.DiscoverError):
        discover.download("https://example.com/a.mp3", "/tmp", "a")


def test_licence_names_and_unknown_requests():
    assert discover._cc("https://creativecommons.org/licenses/by-nc/4.0/") == "CC BY-NC 4.0"
    assert discover._cc("http://creativecommons.org/publicdomain/zero/1.0/") == "CC0"
    assert discover._cc("https://example.com/terms") == ""
    assert discover.search("openverse", "   ") == {"items": [], "more": False}
    with pytest.raises(discover.DiscoverError):
        discover.search("nowhere", "bell")
    with pytest.raises(discover.DiscoverError):
        discover.search("freesound", "bell", 1, "")
    with pytest.raises(discover.DiscoverError):
        discover.waveform("../../etc/passwd")


def test_restore_brings_regions_back(lib, add, tmp_path):
    a = add("a.wav", tags=["x"])
    lib.regions_set(a, [{"start": 0.01, "end": 0.05, "name": "hit"}])
    file = lib.export_backup(str(tmp_path))
    lib.regions_set(a, [])
    assert lib.restore_backup(file)["regions"] == 1
    assert [r["name"] for r in lib.regions_get(a)] == ["hit"]


def test_restore_reads_the_dated_backups_too(lib, add, tmp_path):
    pid = lib.create_project("kit")["id"]
    a = add("a.wav", seed=1, tags=["keep"], project=pid)
    b = add("b.wav", seed=2, project=pid)
    dated = {"catalog": [dict(r) for r in lib.q("SELECT path, tags, memo, rating, updated FROM catalog")],
             "projects": [{"id": pid, "name": "kit"}],
             "project_items": [dict(r) for r in lib.q("SELECT project_id, path, added FROM project_items")]}
    file = tmp_path / "catalog-2026-11-13.json"
    file.write_text(json.dumps(dated))
    lib.delete_project(pid)
    lib.catalog_set(a, {"tags": []})
    assert lib.restore_backup(str(file))["projects"] == 1
    assert lib.catalog_get(a)["tags"] == ["keep"]
    assert lib.project_paths(lib.projects()[0]["id"]) == {a, b}


def test_a_damaged_backup_restores_nothing(lib, add, tmp_path):
    a = add("a.wav", tags=["mine"])
    bad = tmp_path / "bad.json"
    bad.write_text(json.dumps({"catalog": [{"path": a, "tags": ["theirs"], "rating": 3}, "not a row"], "projects": [{"name": "kit", "paths": ["x"]}]}))
    with pytest.raises(ValueError):
        lib.restore_backup(str(bad))
    assert lib.catalog_get(a)["tags"] == ["mine"] and lib.projects() == []


def test_a_project_folder_that_left_the_library_rejoins_it(lib, add, tmp_path):
    src = add("kick.wav")
    kit = os.path.realpath(tmp_path / "elsewhere")
    os.mkdir(kit)
    pid = lib.create_project("kit")["id"]
    lib.x("UPDATE projects SET folder=? WHERE id=?", (kit, pid))
    assert kit not in lib.folders()
    lib.project_add(pid, src)
    assert kit in lib.folders()


def test_same_name_and_size_but_different_sound_is_not_reused(lib, add):
    a = add("one/kick.wav", seed=1)
    b = add("two/kick.wav", seed=2)
    assert os.path.getsize(a) == os.path.getsize(b)
    kit = os.path.join(lib.folder, "kit")
    os.mkdir(kit)
    pid = lib.create_project("kit")["id"]
    lib.set_project_folder(pid, kit)
    first, second = lib.project_add(pid, a), lib.project_add(pid, b)
    assert first != second and len(os.listdir(kit)) == 2


def test_setting_a_folder_keeps_the_order(lib, add):
    pid = lib.create_project("kit")["id"]
    a, b = add("a.wav", seed=1, project=pid), add("b.wav", seed=2, project=pid)
    lib.set_project_order(pid, [b, a])
    kit = os.path.join(lib.folder, "kit")
    os.mkdir(kit)
    lib.set_project_folder(pid, kit)
    assert [os.path.basename(p) for p in lib.project_order(pid)] == ["b.wav", "a.wav"]


def test_names_never_start_with_a_dot():
    from athar.library import safe_name
    assert safe_name(".hidden take") == "hidden take"
    assert safe_name(".") == "untitled"
    assert safe_name("..") == "untitled"


def test_a_folder_made_by_hand_in_the_projects_folder_becomes_a_project(lib, add, tmp_path):
    base = os.path.join(lib.folder, "projects")
    os.makedirs(os.path.join(base, "by hand"))
    lib.update_settings({"projects_dir": base})
    inside = add("projects/by hand/a_b.wav")
    lib.adopt_project_folders()
    p = next(p for p in lib.projects() if p["name"] == "by hand")
    assert p["folder"] == os.path.join(base, "by hand") and lib.project_paths(p["id"]) == {inside}
    # Deleted in the app, it stays deleted although its folder is still on disk.
    lib.delete_project(p["id"])
    lib.adopt_project_folders()
    assert not any(q["name"] == "by hand" for q in lib.projects())
