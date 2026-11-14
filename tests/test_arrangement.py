import os

import pytest


def clip(path, **over):
    return {"path": path, "name": "x", "duration": 1.0, "sr": 48000, "at": 0.5, "in": 0.1, "out": 0.2} | over


def lane(*clips, **over):
    return {"name": "Drums", "gain": -3, "mute": False, "solo": True, "clips": list(clips)} | over


def test_arrangement_is_kept_and_starts_empty(lib, add):
    assert lib.arrangement_get()["lanes"] == []
    path = add("kick.wav")
    lib.arrangement_set({"loop": False, "lanes": [lane(clip(path))]})
    a = lib.arrangement_get()
    assert a["loop"] is False
    assert a["lanes"] == [lane(clip(path, name="kick.wav"), gain=-3.0)]


def test_arrangement_values_are_held_in_range(lib, add):
    path = add("kick.wav")
    a = lib.arrangement_set({"lanes": [lane(clip(path, at=-1), clip(path, at=1e9, **{"in": 0.8, "out": 0.9}), gain=40, name="  ")] + [lane()] * 40})
    assert a["loop"] is True and len(a["lanes"]) == 16
    first = a["lanes"][0]
    assert (first["gain"], first["name"], [c["at"] for c in first["clips"]]) == (6, "Lane 1", [0, 600])
    # Fades that would meet in the middle are cut back to the length of the clip.
    assert first["clips"][1]["in"] == 0.8 and abs(first["clips"][1]["out"] - 0.2) < 1e-9
    assert a["lanes"][1]["clips"] == []


def test_arrangement_rejects_a_clip_without_a_path(lib):
    with pytest.raises(KeyError):
        lib.arrangement_set({"lanes": [{"clips": [{"at": 0}]}]})


def test_an_arrangement_of_one_sound_per_lane_becomes_lanes_of_clips(lib, add):
    path = add("kick.wav")
    old = {"bpm": 120, "grid": False, "lanes": [{"path": path, "name": "kick.wav", "duration": 0.1, "sr": 48000, "gain": -6, "mute": True, "solo": False, "hits": [0, 0.5]}]}
    lib.x("INSERT OR REPLACE INTO meta VALUES ('arrangement', ?)", (__import__("json").dumps(old),))
    a = lib.arrangement_get()
    assert [(x["name"], x["gain"], x["mute"]) for x in a["lanes"]] == [("kick", -6, True)]
    assert [(c["path"], c["at"], c["in"], c["out"]) for c in a["lanes"][0]["clips"]] == [(path, 0, 0.0, 0.0), (path, 0.5, 0.0, 0.0)]


def test_arrangement_follows_a_renamed_file(lib, add):
    path, other = add("kick.wav", seed=1), add("snare.wav", seed=2)
    lib.arrangement_set({"lanes": [lane(clip(path), clip(other))]})
    new = lib.rename(path, "boom")
    assert [(c["path"], c["name"]) for c in lib.arrangement_get()["lanes"][0]["clips"]] == [(new, "boom.wav"), (other, "snare.wav")]


def test_arrangement_follows_a_moved_folder(lib, add):
    path = add("drums/kick.wav")
    lib.arrangement_set({"lanes": [lane(clip(path))]})
    moved = os.path.join(lib.folder, "kit")
    os.rename(os.path.join(lib.folder, "drums"), moved)
    lib.move(os.path.join(lib.folder, "drums"), moved, is_dir=True)
    assert lib.arrangement_get()["lanes"][0]["clips"][0]["path"] == os.path.join(moved, "kick.wav")
