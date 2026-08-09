"""Coverage completion tests for core/adaptive_weights.py (WQS weight calibrator)."""

import json
from unittest.mock import patch

import pytest

from core.adaptive_weights import (
    DEFAULT_WQS_WEIGHTS,
    AdaptiveWeightCalibrator,
    get_effective_wqs_weights,
)
from core.correlation_reader import WqsCorrelationRecord


def make_record(pnl, roi_val, strategy="SHIELD"):
    """Build a WqsCorrelationRecord with a components JSON blob."""
    return WqsCorrelationRecord(
        wallet_address=f"wallet_{pnl}_{strategy}",
        wqs_score_at_promotion=60.0,
        actual_copy_pnl_7d_sol=pnl,
        actual_copy_pnl_30d_sol=pnl,
        actual_copy_pnl_all_sol=pnl,
        copy_trade_count_7d=5,
        copy_trade_count_30d=10,
        copy_trade_count_all=20,
        strategy=strategy,
        wqs_components_json=json.dumps({"roi_score": roi_val}),
        promoted_at="2026-01-01",
        last_updated_at="2026-01-01",
    )


def make_fake_reader(records):
    """CorrelationReader stand-in with a real Pearson implementation."""

    class FakeReader:
        calls = []

        def __init__(self, db_path=None):
            pass

        def get_all_records(self, strategy=None, min_trades=0):
            FakeReader.calls.append((strategy, min_trades))
            return records

        @staticmethod
        def _pearson_correlation(xs, ys):
            n = min(len(xs), len(ys))
            if n < 3:
                return 0.0
            mx = sum(xs) / n
            my = sum(ys) / n
            num = sum((xs[i] - mx) * (ys[i] - my) for i in range(n))
            dx = sum((v - mx) ** 2 for v in xs) ** 0.5
            dy = sum((v - my) ** 2 for v in ys) ** 0.5
            if dx == 0 or dy == 0:
                return 0.0
            return num / (dx * dy)

    return FakeReader


def make_calibrator(tmp_path):
    db_path = tmp_path / "chimera.db"
    return AdaptiveWeightCalibrator(db_path=str(db_path))


class TestGetCurrentWeights:
    def test_no_cache_file_returns_defaults(self, tmp_path):
        cal = make_calibrator(tmp_path)
        assert cal.get_current_weights() == dict(DEFAULT_WQS_WEIGHTS)

    def test_cache_file_valid(self, tmp_path):
        cal = make_calibrator(tmp_path)
        cache_file = tmp_path / "wqs_adaptive_weights.json"
        cache_file.write_text(json.dumps({"roi_score": 1.9, "activity_score": 0.6}))
        weights = cal.get_current_weights()
        assert weights["roi_score"] == 1.9
        assert weights["activity_score"] == 0.6

    def test_cache_file_invalid_json(self, tmp_path):
        cal = make_calibrator(tmp_path)
        cache_file = tmp_path / "wqs_adaptive_weights.json"
        cache_file.write_text("{corrupt")
        assert cal.get_current_weights() == dict(DEFAULT_WQS_WEIGHTS)

    def test_cache_file_empty_dict(self, tmp_path):
        cal = make_calibrator(tmp_path)
        cache_file = tmp_path / "wqs_adaptive_weights.json"
        cache_file.write_text(json.dumps({}))
        assert cal.get_current_weights() == dict(DEFAULT_WQS_WEIGHTS)


class TestCalibrate:
    def test_insufficient_records(self, tmp_path):
        cal = make_calibrator(tmp_path)
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader([])):
            assert cal.calibrate() is None

    def test_all_records_skip_pnl(self, tmp_path):
        """Records with missing pnl/components produce no pairs -> None."""
        cal = make_calibrator(tmp_path)
        records = [
            WqsCorrelationRecord(
                wallet_address="w1", wqs_score_at_promotion=50.0,
                actual_copy_pnl_7d_sol=None, actual_copy_pnl_30d_sol=None,
                actual_copy_pnl_all_sol=None, copy_trade_count_7d=0,
                copy_trade_count_30d=0, copy_trade_count_all=1,
                strategy="SHIELD", wqs_components_json=None,
                promoted_at="2026-01-01", last_updated_at="2026-01-01",
            )
        ]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            assert cal.calibrate() is None

    def test_three_records_missing_pnl_returns_none(self, tmp_path):
        """>= MIN_SAMPLES records but every row skipped -> None at pnl_vals check."""
        cal = make_calibrator(tmp_path)
        records = [
            make_record(None, 5.0),
            make_record(None, 6.0),
            make_record(None, 7.0),
        ]
        for r in records:
            r.actual_copy_pnl_30d_sol = None
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            assert cal.calibrate() is None

    def test_skip_non_dict_components(self, tmp_path):
        """A JSON-list components blob is skipped, valid records still count."""
        cal = make_calibrator(tmp_path)
        bad = make_record(1.0, 5.0)
        bad.wqs_components_json = "[1, 2, 3]"
        records = [bad] + [make_record(float(i), 5.0 + i) for i in range(4)]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None

    def test_skip_non_numeric_component_value(self, tmp_path):
        """Non-numeric component values are skipped, valid ones still count."""
        cal = make_calibrator(tmp_path)
        bad = make_record(1.0, "high")
        records = [bad] + [make_record(float(i), 5.0 + i) for i in range(4)]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None

    def test_skip_corrupt_json_record(self, tmp_path):
        """Corrupt components JSON is skipped, valid records still count."""
        cal = make_calibrator(tmp_path)
        bad = make_record(1.0, 5.0)
        bad.wqs_components_json = "{corrupt"
        records = [bad] + [make_record(float(i), 5.0 + i) for i in range(4)]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None

    def test_component_with_too_few_pairs_skipped(self, tmp_path):
        """Components with fewer than MIN_SAMPLES pairs are skipped."""
        cal = make_calibrator(tmp_path)
        records = [
            make_record(1.0, 1.0),
            make_record(2.0, 2.0),
            make_record(3.0, 3.0),
            make_record(4.0, 4.0),
        ]
        records[0].wqs_components_json = json.dumps({"roi_score": 1.0, "solo": 9.0})
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None
        assert "solo" in weights

    def test_noise_component_reset_to_neutral(self, tmp_path):
        """Components with noise-level correlation reset to 1.0."""
        cal = make_calibrator(tmp_path)
        noise = [5.0, 1.0, 9.0, 3.0, 7.0, 2.0]
        records = [
            make_record(float(i), float(i)) for i in range(6)
        ]
        for r, n in zip(records, noise):
            r.wqs_components_json = json.dumps(
                {"roi_score": r.actual_copy_pnl_30d_sol, "noise": n}
            )
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.5)
        assert weights is not None
        assert weights["noise"] == 1.0

    def test_invalid_components_json(self, tmp_path):
        cal = make_calibrator(tmp_path)
        bad = make_record(1.0, 5.0)
        bad.wqs_components_json = "{not-json"
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader([bad])):
            assert cal.calibrate() is None

    def test_components_not_dict(self, tmp_path):
        cal = make_calibrator(tmp_path)
        record = make_record(1.0, 5.0)
        record.wqs_components_json = "[1, 2, 3]"
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader([record])):
            assert cal.calibrate() is None

    def test_non_numeric_component_values(self, tmp_path):
        cal = make_calibrator(tmp_path)
        record = make_record(1.0, "high")
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader([record])):
            assert cal.calibrate() is None

    def test_correlations_below_min_correlation(self, tmp_path):
        """All correlations under the threshold -> no component_corrs -> None."""
        cal = make_calibrator(tmp_path)
        records = []
        for i in range(6):
            records.append(make_record(float(i), [5, 1, 9, 3, 7, 2][i]))
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            assert cal.calibrate(min_correlation=0.99) is None

    def test_bayesian_blend_early(self, tmp_path):
        """n_records <= MIN_SAMPLES: confidence 0.0, conservative 70/30 blend."""
        cal = make_calibrator(tmp_path)
        records = [make_record(float(i), 10.0 + i) for i in range(3)]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None
        seed = DEFAULT_WQS_WEIGHTS["roi_score"]
        assert weights["roi_score"] == pytest.approx(seed * 0.7 + (1.0 + 1.0) * 0.3)

    def test_confidence_blend_mid(self, tmp_path):
        """n_records between BAYESIAN_THRESHOLD and WARM_START: blended path."""
        cal = make_calibrator(tmp_path)
        records = [make_record(float(i), float(i)) for i in range(12)]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None
        assert 0.5 <= weights["roi_score"] <= 2.0

    def test_ema_blend_mature(self, tmp_path):
        """n_records >= WARM_START_SAMPLES: standard 30/70 EMA blend."""
        cal = make_calibrator(tmp_path)
        records = [make_record(float(i), float(i)) for i in range(16)]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None
        assert weights["roi_score"] == pytest.approx(
            DEFAULT_WQS_WEIGHTS["roi_score"] * 0.7 + 2.0 * 0.3
        )

    def test_negative_correlation_weights_down(self, tmp_path):
        """Negative correlation -> weight below 1.0 (clamped at MIN_WEIGHT)."""
        cal = make_calibrator(tmp_path)
        records = [make_record(float(i), 50.0 - float(i)) for i in range(6)]
        with patch("core.correlation_reader.CorrelationReader", make_fake_reader(records)):
            weights = cal.calibrate(min_correlation=0.0)
        assert weights is not None
        assert weights["roi_score"] < DEFAULT_WQS_WEIGHTS["roi_score"]

    def test_strategy_passed_to_reader(self, tmp_path):
        cal = make_calibrator(tmp_path)
        fake_cls = make_fake_reader([])
        with patch("core.correlation_reader.CorrelationReader", fake_cls):
            cal.calibrate(strategy="SPEAR")
        assert fake_cls.calls == [("SPEAR", 1)]


class TestSaveWeights:
    def test_save_weights_writes_file(self, tmp_path):
        cal = make_calibrator(tmp_path)
        cal.save_weights({"roi_score": 1.5, "activity_score": 0.7})
        saved = json.loads((tmp_path / "wqs_adaptive_weights.json").read_text())
        assert saved["roi_score"] == 1.5


class TestShouldCalibrate:
    def test_counter_increments(self, tmp_path):
        cal = make_calibrator(tmp_path)
        assert cal.should_calibrate(run_interval=3) is False
        assert cal.should_calibrate(run_interval=3) is False
        assert cal.should_calibrate(run_interval=3) is True

    def test_counter_invalid_content(self, tmp_path):
        cal = make_calibrator(tmp_path)
        counter = tmp_path / "wqs_calibration_counter.txt"
        counter.write_text("not-a-number")
        assert cal.should_calibrate(run_interval=2) is False
        assert cal.should_calibrate(run_interval=2) is True


class TestCalibrateIfNeeded:
    def test_runs_and_saves(self, tmp_path):
        cal = make_calibrator(tmp_path)
        records = [make_record(float(i), float(i)) for i in range(6)]
        with patch.object(cal, "should_calibrate", return_value=True), patch(
            "core.correlation_reader.CorrelationReader", make_fake_reader(records)
        ):
            weights = cal.calibrate_if_needed()
        assert weights is not None
        assert (tmp_path / "wqs_adaptive_weights.json").exists()

    def test_calibrate_returns_none(self, tmp_path):
        cal = make_calibrator(tmp_path)
        with patch.object(cal, "should_calibrate", return_value=True), patch.object(
            cal, "calibrate", return_value=None
        ), patch.object(cal, "save_weights") as mock_save:
            assert cal.calibrate_if_needed() is None
            mock_save.assert_not_called()

    def test_not_due(self, tmp_path):
        cal = make_calibrator(tmp_path)
        with patch.object(cal, "should_calibrate", return_value=False), patch.object(
            cal, "calibrate"
        ) as mock_calibrate:
            assert cal.calibrate_if_needed() is None
            mock_calibrate.assert_not_called()


class TestGetEffectiveWeights:
    def test_returns_defaults(self, tmp_path, monkeypatch):
        monkeypatch.setenv("CHIMERA_DB_PATH", str(tmp_path / "chimera.db"))
        weights = get_effective_wqs_weights()
        assert weights == dict(DEFAULT_WQS_WEIGHTS)
