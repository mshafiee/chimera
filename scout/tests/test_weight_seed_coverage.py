"""
Coverage tests for core/weight_seed.py.

Covers the merge/skip/fallback branches of load_seeded_weights and the
env-var override in get_weights_path.
"""

import json

import core.weight_seed as ws


def test_get_weights_path_default():
    assert ws.get_weights_path() == ws._DEFAULT_WEIGHTS_PATH


def test_get_weights_path_env_override(monkeypatch):
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", "/tmp/custom.json")
    assert ws.get_weights_path() == "/tmp/custom.json"


def test_load_seeded_weights_merges_over_defaults(monkeypatch, tmp_path):
    path = tmp_path / "weights.json"
    path.write_text(json.dumps({"roi_score": 9.5}))
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(path))
    weights = ws.load_seeded_weights()
    assert weights["roi_score"] == 9.5
    # Every fallback component still present after merge
    assert set(ws._FALLBACK_WEIGHTS) <= set(weights)


def test_load_seeded_weights_skips_non_numeric(monkeypatch, tmp_path):
    path = tmp_path / "weights.json"
    path.write_text(json.dumps({"roi_score": "oops", "win_rate_score": 1.1}))
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(path))
    weights = ws.load_seeded_weights()
    assert weights["roi_score"] == ws._FALLBACK_WEIGHTS["roi_score"]
    assert weights["win_rate_score"] == 1.1


def test_load_seeded_weights_no_valid_entries(monkeypatch, tmp_path):
    path = tmp_path / "weights.json"
    path.write_text(json.dumps({"roi_score": "oops"}))
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(path))
    weights = ws.load_seeded_weights()
    assert weights == dict(ws._FALLBACK_WEIGHTS)


def test_load_seeded_weights_non_dict(monkeypatch, tmp_path):
    path = tmp_path / "weights.json"
    path.write_text("[1, 2, 3]")
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(path))
    weights = ws.load_seeded_weights()
    assert weights == dict(ws._FALLBACK_WEIGHTS)


def test_load_seeded_weights_missing_file(monkeypatch, tmp_path):
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(tmp_path / "missing.json"))
    weights = ws.load_seeded_weights()
    assert weights == dict(ws._FALLBACK_WEIGHTS)


def test_load_seeded_weights_corrupt_json(monkeypatch, tmp_path):
    path = tmp_path / "weights.json"
    path.write_text("{ not valid json !!")
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(path))
    weights = ws.load_seeded_weights()
    assert weights == dict(ws._FALLBACK_WEIGHTS)


def test_load_seeded_weights_encoding_error(monkeypatch, tmp_path):
    path = tmp_path / "weights.json"
    path.write_bytes(b"\xff\xfe\x00 invalid utf8 \x80")
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(path))
    weights = ws.load_seeded_weights()
    assert weights == dict(ws._FALLBACK_WEIGHTS)


def test_load_seeded_weights_oserror(monkeypatch, tmp_path):
    # A directory path raises IsADirectoryError (an OSError) on open()
    monkeypatch.setenv("SCOUT_WQS_WEIGHTS_PATH", str(tmp_path))
    weights = ws.load_seeded_weights()
    assert weights == dict(ws._FALLBACK_WEIGHTS)
