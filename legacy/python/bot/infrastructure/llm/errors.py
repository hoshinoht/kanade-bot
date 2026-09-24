"""Model-gateway failures. Messages never carry key material or response bodies."""

from __future__ import annotations


class ModelError(RuntimeError):
    """Any failure to get an answer from the model gateway."""


class ModelConfigError(ModelError):
    """A required model setting or the key file is missing or unusable."""


class ModelUnavailable(ModelError):
    """The gateway could not be reached or refused/failed the request."""


class ModelAuthError(ModelUnavailable):
    """The gateway rejected the bearer key (HTTP 401/403)."""


class ModelFieldRejected(ModelUnavailable):
    """HTTP 400 naming one request field in ``error.param``; ``param`` is that name."""

    def __init__(self, message: str, param: str):
        super().__init__(message)
        self.param = param


class ModelTimeout(ModelUnavailable, TimeoutError):
    """The gateway did not answer in time; also a :class:`TimeoutError`."""


class ModelResponseError(ModelError):
    """The gateway answered with something that is not a chat completion."""
