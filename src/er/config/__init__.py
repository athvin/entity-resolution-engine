"""Config document loading, validation and hashing (DesignDoc.md S6, S5.2).

The three modules split along the spec's own seams: :mod:`er.config.schema` is S6
and S6.1, :mod:`er.config.loader` is the S4.0 start-up step that reads and
validates, and :mod:`er.config.hashing` is the S5.2 digest. Callers import the
package.
"""

from er.config.hashing import canonical_document, config_hash
from er.config.loader import CONFIG_ENV_VAR, ConfigError, ConfigValidationError, load_config
from er.config.schema import CONFIG_BLOCKS, Config, normalize

__all__ = [
    "CONFIG_BLOCKS",
    "CONFIG_ENV_VAR",
    "Config",
    "ConfigError",
    "ConfigValidationError",
    "canonical_document",
    "config_hash",
    "load_config",
    "normalize",
]
