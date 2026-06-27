"""
Scout Integration Modules

This package contains integration layers that connect Scout's core components
with external systems and optimization modules.
"""

from .high_conviction_integration import (
    HighConvictionIntegration,
    create_high_conviction_integration,
)

__all__ = [
    "HighConvictionIntegration",
    "create_high_conviction_integration",
]