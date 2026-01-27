"""Configuration classes for the Flask application."""

import os
from datetime import timedelta


class Config:
    """Base configuration."""

    SECRET_KEY = os.environ.get('SECRET_KEY', 'dev_secret_key')
    PERMANENT_SESSION_LIFETIME = timedelta(days=30)

    # Session cookie settings - Lax works for same-site navigations
    SESSION_COOKIE_SAMESITE = 'Lax'
    SESSION_COOKIE_SECURE = False
    SESSION_COOKIE_HTTPONLY = True

    # File paths
    TODO_PATH = os.environ.get('TODOS_DB_PATH', 'TodosDatenbank.md')
    CONFIG_PATH = os.environ.get('CONFIG_PATH', '/config/settings.json')

    # AI settings
    DEFAULT_AI_TIMEOUT_SECS = int(os.environ.get('AI_TIMEOUT_SECS', '30'))
    RECENT_CONTEXT_WINDOW_DAYS = 30

    # WebDAV Configuration
    USE_WEBDAV = os.environ.get('USE_WEBDAV', 'false').lower() == 'true'
    WEBDAV_URL = os.environ.get('WEBDAV_URL')
    WEBDAV_USERNAME = os.environ.get('WEBDAV_USERNAME')
    WEBDAV_PASSWORD = os.environ.get('WEBDAV_PASSWORD')

    # OIDC Configuration
    OIDC_ISSUER = os.environ.get('OIDC_ISSUER')
    OIDC_CLIENT_ID = os.environ.get('OIDC_CLIENT_ID')
    OIDC_CLIENT_SECRET = os.environ.get('OIDC_CLIENT_SECRET')
    OIDC_REDIRECT_URI = os.environ.get('OIDC_REDIRECT_URI')
    OIDC_ALLOWED_USER = os.environ.get('OIDC_ALLOWED_USER', '').lower()

    # App authentication
    APP_USER = os.environ.get('APP_USER')
    APP_PASSWORD = os.environ.get('APP_PASSWORD')

    # LLM settings
    OLLAMA_URL = os.environ.get('OLLAMA_URL')
    OLLAMA_MODEL = os.environ.get('OLLAMA_MODEL')
    OLLAMA_PROMPT_STYLE = os.environ.get('OLLAMA_PROMPT_STYLE', 'verbose')  # 'verbose' or 'minimal'

    # AI Debug mode (admin-only)
    AI_DEBUG_ENABLED = os.environ.get('AI_DEBUG_ENABLED', 'false').lower() == 'true'

    # Logging
    LOG_LEVEL = os.environ.get('LOG_LEVEL', 'INFO').upper()


class DevelopmentConfig(Config):
    """Development configuration."""

    DEBUG = True
    SESSION_COOKIE_SECURE = False


class ProductionConfig(Config):
    """Production configuration."""

    DEBUG = False
    SESSION_COOKIE_SECURE = True
    SESSION_COOKIE_SAMESITE = 'None'  # Required for cross-site OAuth redirects


class TestingConfig(Config):
    """Testing configuration."""

    TESTING = True
    WTF_CSRF_ENABLED = False
    SESSION_COOKIE_SECURE = False


config = {
    'development': DevelopmentConfig,
    'production': ProductionConfig,
    'testing': TestingConfig,
    'default': DevelopmentConfig
}
