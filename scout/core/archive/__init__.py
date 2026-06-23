"""
Archive — experimental/legacy modules not wired into the production Scout pipeline.

These modules are preserved for reference but are NOT imported by any production
code path. They use real ML libraries (xgboost, lightgbm, sklearn, shap) that
are optional dependencies.

To re-activate a module, move it back to scout/core/ and update any absolute
imports from scout.core.archive.X to scout.core.X.
"""
