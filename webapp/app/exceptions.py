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
