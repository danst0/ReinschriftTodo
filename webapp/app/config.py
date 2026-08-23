"""Configuration classes for the Flask application."""

import os
import tempfile
from datetime import timedelta


class Config:
    """Base configuration."""

    SECRET_KEY = os.environ.get('SECRET_KEY', 'dev_secret_key')
    PERMANENT_SESSION_LIFETIME = timedelta(days=30)

    # The CSRF token is baked into the page when it renders and the 30 s partial
    # reloads never replace it. With Flask-WTF's default hour, a page left open
    # kept looking current but could no longer write: every POST came back 400,
    # and a tapped checkbox silently reappeared unticked. The token stays signed
    # and bound to the session, so the session is the lifetime that matters.
    WTF_CSRF_TIME_LIMIT = None

    # Session cookie settings - Lax works for same-site navigations
    SESSION_COOKIE_SAMESITE = 'Lax'
    SESSION_COOKIE_SECURE = False
    SESSION_COOKIE_HTTPONLY = True

    # File paths
    TODO_PATH = os.environ.get('TODOS_DB_PATH', 'TodosDatenbank.md')
    CONFIG_PATH = os.environ.get('CONFIG_PATH', '/config/settings.json')

    # Server-side sessions: the undo stack stores full file snapshots, which
    # overflow the ~4 KB client cookie limit. Store session data on disk and
    # keep only the session id in the cookie. SESSION_DIR defaults next to
    # CONFIG_PATH so it lives on the persistent /config volume in Docker; the
    # cachelib backend is wired up in init_extensions().
    SESSION_TYPE = 'cachelib'
    SESSION_DIR = os.environ.get(
        'SESSION_FILE_DIR',
        os.path.join(os.path.dirname(CONFIG_PATH) or '.', 'flask_session'),
    )
    SESSION_PERMANENT = True

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

    # Semantic suggestions via Ollama embeddings (opt-in)
    SEMANTIC_ENABLED = os.environ.get('SEMANTIC_ENABLED', 'false').lower() == 'true'
    EMBEDDING_MODEL = os.environ.get('EMBEDDING_MODEL', 'bge-m3')
    EMBEDDING_TIMEOUT = int(os.environ.get('EMBEDDING_TIMEOUT', '30'))
    EMBEDDING_CACHE_PATH = os.environ.get(
        'EMBEDDING_CACHE_PATH',
        os.path.join(os.path.dirname(CONFIG_PATH) or '.', 'embeddings.json'),
    )

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
    SESSION_DIR = os.path.join(tempfile.gettempdir(), 'reinschrift_test_sessions')


config = {
    'development': DevelopmentConfig,
    'production': ProductionConfig,
    'testing': TestingConfig,
    'default': DevelopmentConfig
}
