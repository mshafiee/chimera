"""
Model Registry for Scout

Manages model versioning, A/B testing, and rollback capability.
This module provides:
- Model version tracking with metadata
- A/B testing framework
- Rollback capability for failed deployments
- Model lifecycle management

Usage:
    registry = ModelRegistry()
    registry.register_model(model, version, metadata)
    model = registry.get_model(model_name, version='latest')
"""

import json
import logging
import os
import shutil
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any, Union
from dataclasses import dataclass, asdict
from enum import Enum
import hashlib

logger = logging.getLogger(__name__)


class ModelStatus(Enum):
    """Model lifecycle status."""
    TRAINING = "training"
    STAGING = "staging"
    PRODUCTION = "production"
    DEPRECATED = "deprecated"
    FAILED = "failed"


@dataclass
class ModelMetadata:
    """Metadata for a registered model."""
    model_name: str
    version: str
    model_type: str  # "xgboost", "lightgbm", "meta_learner", etc.
    status: str
    created_at: str
    file_path: str
    file_hash: str
    training_samples: int
    validation_metrics: Dict[str, float]
    hyperparameters: Dict[str, Any]
    tags: List[str]
    parent_version: Optional[str] = None
    description: Optional[str] = None
    ab_test_group: Optional[str] = None  # "A" or "B"
    is_default: bool = False


class ModelRegistry:
    """
    Registry for ML model versioning and lifecycle management.

    Features:
    - Model version tracking
    - A/B testing support
    - Rollback capability
    - Model promotion (staging → production)
    - Model deprecation
    """

    def __init__(self, registry_dir: Optional[str] = None):
        """
        Initialize the model registry.

        Args:
            registry_dir: Directory for model storage
        """
        if registry_dir is None:
            registry_dir = os.getenv("SCOUT_MODEL_REGISTRY_DIR", "../models/registry")

        self.registry_dir = Path(registry_dir)
        self.registry_dir.mkdir(parents=True, exist_ok=True)

        # Subdirectories
        self.models_dir = self.registry_dir / "models"
        self.models_dir.mkdir(exist_ok=True)

        self.metadata_dir = self.registry_dir / "metadata"
        self.metadata_dir.mkdir(exist_ok=True)

        self.ab_tests_dir = self.registry_dir / "ab_tests"
        self.ab_tests_dir.mkdir(exist_ok=True)

        # Index file
        self.index_file = self.registry_dir / "index.json"

        # Load existing index
        self.index = self._load_index()

        # A/B test tracking
        self.ab_tests = {}
        self._load_ab_tests()

    def _load_index(self) -> Dict[str, Any]:
        """Load the model index."""
        if self.index_file.exists():
            try:
                with open(self.index_file, 'r') as f:
                    return json.load(f)
            except Exception as e:
                logger.warning(f"Failed to load index: {e}")

        return {
            'models': {},
            'default_versions': {},
            'last_updated': None,
        }

    def _save_index(self):
        """Save the model index."""
        try:
            self.index['last_updated'] = datetime.utcnow().isoformat()
            with open(self.index_file, 'w') as f:
                json.dump(self.index, f, indent=2)
        except Exception as e:
            logger.error(f"Failed to save index: {e}")

    def _load_ab_tests(self):
        """Load A/B test configurations."""
        ab_test_file = self.ab_tests_dir / "config.json"
        if ab_test_file.exists():
            try:
                with open(ab_test_file, 'r') as f:
                    self.ab_tests = json.load(f)
            except Exception as e:
                logger.warning(f"Failed to load A/B tests: {e}")

    def _save_ab_tests(self):
        """Save A/B test configurations."""
        try:
            ab_test_file = self.ab_tests_dir / "config.json"
            with open(ab_test_file, 'w') as f:
                json.dump(self.ab_tests, f, indent=2)
        except Exception as e:
            logger.error(f"Failed to save A/B tests: {e}")

    def register_model(
        self,
        model_name: str,
        version: str,
        model_file_path: str,
        model_type: str,
        training_samples: int,
        validation_metrics: Dict[str, float],
        hyperparameters: Dict[str, Any],
        tags: Optional[List[str]] = None,
        description: Optional[str] = None,
        parent_version: Optional[str] = None,
        set_as_default: bool = False
    ) -> ModelMetadata:
        """
        Register a model in the registry.

        Args:
            model_name: Name of the model
            version: Version string (e.g., "1.0.0")
            model_file_path: Path to the model file
            model_type: Type of model
            training_samples: Number of training samples
            validation_metrics: Validation performance metrics
            hyperparameters: Model hyperparameters
            tags: Optional tags for categorization
            description: Optional description
            parent_version: Parent version if this is a fine-tune
            set_as_default: Whether to set as default version

        Returns:
            ModelMetadata object
        """
        # Calculate file hash
        file_hash = self._calculate_hash(model_file_path)

        # Copy model to registry
        registry_path = self.models_dir / f"{model_name}_{version.replace('.', '_')}{Path(model_file_path).suffix}"
        shutil.copy2(model_file_path, registry_path)

        # Create metadata
        metadata = ModelMetadata(
            model_name=model_name,
            version=version,
            model_type=model_type,
            status=ModelStatus.STAGING.value,
            created_at=datetime.utcnow().isoformat(),
            file_path=str(registry_path),
            file_hash=file_hash,
            training_samples=training_samples,
            validation_metrics=validation_metrics,
            hyperparameters=hyperparameters,
            tags=tags or [],
            parent_version=parent_version,
            description=description,
            is_default=set_as_default,
        )

        # Save metadata
        metadata_file = self.metadata_dir / f"{model_name}_{version.replace('.', '_')}.json"
        with open(metadata_file, 'w') as f:
            json.dump(asdict(metadata), f, indent=2)

        # Update index
        if model_name not in self.index['models']:
            self.index['models'][model_name] = {}

        self.index['models'][model_name][version] = {
            'status': metadata.status,
            'created_at': metadata.created_at,
            'file_path': str(registry_path),
        }

        if set_as_default:
            self.index['default_versions'][model_name] = version

        self._save_index()

        logger.info(f"Registered model: {model_name} v{version}")
        return metadata

    def get_model(
        self,
        model_name: str,
        version: Optional[str] = None
    ) -> Optional[ModelMetadata]:
        """
        Get a registered model.

        Args:
            model_name: Name of the model
            version: Version string, or None for default/latest

        Returns:
            ModelMetadata object or None
        """
        if version is None:
            version = self.index.get('default_versions', {}).get(model_name)
            if version is None:
                # Get latest version
                if model_name in self.index.get('models', {}):
                    versions = list(self.index['models'][model_name].keys())
                    if versions:
                        version = versions[-1]

        if version is None:
            logger.warning(f"No version found for model: {model_name}")
            return None

        # Load metadata
        metadata_file = self.metadata_dir / f"{model_name}_{version.replace('.', '_')}.json"
        if not metadata_file.exists():
            logger.warning(f"Metadata not found for: {model_name} v{version}")
            return None

        try:
            with open(metadata_file, 'r') as f:
                metadata_dict = json.load(f)
            return ModelMetadata(**metadata_dict)
        except Exception as e:
            logger.error(f"Failed to load metadata: {e}")
            return None

    def promote_to_production(
        self,
        model_name: str,
        version: str,
        rollback_version: Optional[str] = None
    ) -> bool:
        """
        Promote a model to production.

        Args:
            model_name: Name of the model
            version: Version to promote
            rollback_version: Optional version to use for rollback

        Returns:
            True if successful
        """
        metadata = self.get_model(model_name, version)
        if not metadata:
            logger.error(f"Model not found: {model_name} v{version}")
            return False

        # Update status
        metadata.status = ModelStatus.PRODUCTION.value

        # Set as default
        self.index['default_versions'][model_name] = version

        # Save rollback info
        if rollback_version:
            self.index['models'][model_name]['rollback_version'] = rollback_version

        self._save_index()

        # Update metadata file
        metadata_file = self.metadata_dir / f"{model_name}_{version.replace('.', '_')}.json"
        with open(metadata_file, 'w') as f:
            json.dump(asdict(metadata), f, indent=2)

        logger.info(f"Promoted {model_name} v{version} to production")
        return True

    def rollback(
        self,
        model_name: str,
        target_version: Optional[str] = None
    ) -> bool:
        """
        Rollback to a previous version.

        Args:
            model_name: Name of the model
            target_version: Version to rollback to, or None for configured rollback

        Returns:
            True if successful
        """
        if target_version is None:
            target_version = self.index.get('models', {}).get(model_name, {}).get('rollback_version')

        if not target_version:
            logger.error(f"No rollback version configured for: {model_name}")
            return False

        metadata = self.get_model(model_name, target_version)
        if not metadata:
            logger.error(f"Rollback version not found: {model_name} v{target_version}")
            return False

        # Set as default and production
        metadata.status = ModelStatus.PRODUCTION.value
        self.index['default_versions'][model_name] = target_version

        self._save_index()

        logger.info(f"Rolled back {model_name} to v{target_version}")
        return True

    def setup_ab_test(
        self,
        model_name: str,
        version_a: str,
        version_b: str,
        traffic_split: float = 0.1,  # 10% to model B
        duration_days: int = 7
    ) -> bool:
        """
        Set up an A/B test between two model versions.

        Args:
            model_name: Name of the model
            version_a: Control version
            version_b: Treatment version
            traffic_split: Fraction of traffic to version B (0-1)
            duration_days: Test duration in days

        Returns:
            True if successful
        """
        metadata_a = self.get_model(model_name, version_a)
        metadata_b = self.get_model(model_name, version_b)

        if not metadata_a or not metadata_b:
            logger.error(f"One or both models not found for A/B test")
            return False

        # Store A/B test configuration
        test_key = f"{model_name}_ab_test"
        self.ab_tests[test_key] = {
            'model_name': model_name,
            'version_a': version_a,
            'version_b': version_b,
            'traffic_split': traffic_split,
            'start_date': datetime.utcnow().isoformat(),
            'end_date': (datetime.utcnow() + timedelta(days=duration_days)).isoformat(),
            'status': 'active',
            'metrics': {
                'version_a_requests': 0,
                'version_b_requests': 0,
                'version_a_errors': 0,
                'version_b_errors': 0,
            },
        }

        self._save_ab_tests()

        logger.info(f"A/B test set up: {model_name} {version_a} vs {version_b}")
        return True

    def get_ab_test_model(
        self,
        model_name: str,
        sample_id: str
    ) -> Tuple[str, str]:
        """
        Get model version for A/B testing.

        Args:
            model_name: Name of the model
            sample_id: ID to hash for consistent assignment

        Returns:
            Tuple of (version, group)
        """
        test_key = f"{model_name}_ab_test"
        test = self.ab_tests.get(test_key)

        if not test or test.get('status') != 'active':
            # No active A/B test, use default
            default_version = self.index.get('default_versions', {}).get(model_name)
            return (default_version or 'latest', 'default')

        # Hash-based assignment
        hash_val = int(hashlib.sha256(sample_id.encode()).hexdigest(), 16) % 100
        threshold = test['traffic_split'] * 100

        if hash_val < threshold:
            version = test['version_b']
            group = 'B'
        else:
            version = test['version_a']
            group = 'A'

        # Track metrics
        test['metrics'][f'version_{group.lower()}_requests'] += 1
        self._save_ab_tests()

        return (version, group)

    def list_models(
        self,
        model_name: Optional[str] = None,
        status: Optional[str] = None
    ) -> List[ModelMetadata]:
        """
        List registered models.

        Args:
            model_name: Filter by model name
            status: Filter by status

        Returns:
            List of ModelMetadata objects
        """
        results = []

        for metadata_file in self.metadata_dir.glob("*.json"):
            try:
                with open(metadata_file, 'r') as f:
                    metadata_dict = json.load(f)
                metadata = ModelMetadata(**metadata_dict)

                # Apply filters
                if model_name and metadata.model_name != model_name:
                    continue
                if status and metadata.status != status:
                    continue

                results.append(metadata)
            except Exception:
                continue

        # Sort by created date
        results.sort(key=lambda m: m.created_at, reverse=True)
        return results

    def deprecate_model(
        self,
        model_name: str,
        version: str
    ) -> bool:
        """
        Deprecate a model version.

        Args:
            model_name: Name of the model
            version: Version to deprecate

        Returns:
            True if successful
        """
        metadata = self.get_model(model_name, version)
        if not metadata:
            return False

        metadata.status = ModelStatus.DEPRECATED.value

        # Update metadata file
        metadata_file = self.metadata_dir / f"{model_name}_{version.replace('.', '_')}.json"
        with open(metadata_file, 'w') as f:
            json.dump(asdict(metadata), f, indent=2)

        # Update index
        if model_name in self.index.get('models', {}) and version in self.index['models'][model_name]:
            self.index['models'][model_name][version]['status'] = ModelStatus.DEPRECATED.value

        self._save_index()

        logger.info(f"Deprecated {model_name} v{version}")
        return True

    def delete_model(
        self,
        model_name: str,
        version: str,
        force: bool = False
    ) -> bool:
        """
        Delete a model from registry.

        Args:
            model_name: Name of the model
            version: Version to delete
            force: Force delete even if in production

        Returns:
            True if successful
        """
        metadata = self.get_model(model_name, version)
        if not metadata:
            return False

        # Check if in production
        if metadata.status == ModelStatus.PRODUCTION.value and not force:
            logger.error(f"Cannot delete production model without force=True")
            return False

        # Delete files
        try:
            os.remove(metadata.file_path)
            metadata_file = self.metadata_dir / f"{model_name}_{version.replace('.', '_')}.json"
            os.remove(metadata_file)

            # Update index
            if model_name in self.index.get('models', {}) and version in self.index['models'][model_name]:
                del self.index['models'][model_name][version]

            self._save_index()

            logger.info(f"Deleted {model_name} v{version}")
            return True

        except Exception as e:
            logger.error(f"Failed to delete model: {e}")
            return False

    def _calculate_hash(self, file_path: str) -> str:
        """Calculate SHA256 hash of a file."""
        sha256 = hashlib.sha256()
        with open(file_path, 'rb') as f:
            for chunk in iter(lambda: f.read(4096), b''):
                sha256.update(chunk)
        return sha256.hexdigest()

    def get_registry_stats(self) -> Dict[str, Any]:
        """Get registry statistics."""
        models = self.list_models()

        stats = {
            'total_models': len(models),
            'by_status': {},
            'by_type': {},
            'model_names': list(set(m.model_name for m in models)),
            'ab_tests_active': sum(1 for t in self.ab_tests.values() if t.get('status') == 'active'),
        }

        for model in models:
            stats['by_status'][model.status] = stats['by_status'].get(model.status, 0) + 1
            stats['by_type'][model.model_type] = stats['by_type'].get(model.model_type, 0) + 1

        return stats


# Global registry instance
_global_registry = None


def get_model_registry() -> ModelRegistry:
    """Get or create global model registry instance."""
    global _global_registry
    if _global_registry is None:
        _global_registry = ModelRegistry()
    return _global_registry


# Import timedelta for A/B test
from datetime import timedelta
