import traceback

def pytest_configure(config):
    import core.prediction_logger as pl
    orig = pl.logger.error
    def err(msg, *a, **k):
        if 'Failed to log' in str(msg) or 'Failed to ensure' in str(msg):
            print('\n<<TRACE>>\n', traceback.format_exc())
        return orig(msg, *a, **k)
    pl.logger.error = err
