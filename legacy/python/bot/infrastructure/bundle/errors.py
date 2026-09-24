"""Safe, typed failures for portable archive validation."""


class BundleError(ValueError):
    """A validation failure whose code is safe to expose to operators."""

    def __init__(self, code: str) -> None:
        self.code = code
        super().__init__(code)
