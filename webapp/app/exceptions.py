"""Custom exceptions for the todo application."""


class ReinschriftError(Exception):
    """Base exception for Reinschrift application errors."""
    pass


class StorageError(ReinschriftError):
    """Exception raised when storage operations fail."""
    pass


class ParseError(ReinschriftError):
    """Exception raised when parsing fails."""
    pass


class TodoNotFoundError(ReinschriftError):
    """Exception raised when a todo item is not found."""
    pass


class AIParseError(ReinschriftError):
    """Exception raised when AI parsing fails."""
    pass


class AITimeoutError(AIParseError):
    """Exception raised when AI parsing times out."""
    pass


class ConfigurationError(ReinschriftError):
    """Exception raised for configuration errors."""
    pass


class AuthenticationError(ReinschriftError):
    """Exception raised for authentication errors."""
    pass


class ConflictError(ReinschriftError):
    """Exception raised when writing to a file that was modified externally."""

    def __init__(self, local_fingerprint: str = "", remote_fingerprint: str = ""):
        self.local_fingerprint = local_fingerprint
        self.remote_fingerprint = remote_fingerprint
        super().__init__(
            f"Conflict: local fingerprint '{local_fingerprint}' "
            f"does not match remote '{remote_fingerprint}'"
        )
