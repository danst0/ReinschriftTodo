"""Flask extensions initialization."""

from flask_wtf.csrf import CSRFProtect
from flask_session import Session
from authlib.integrations.flask_client import OAuth

# CSRF Protection
csrf = CSRFProtect()

# OAuth client for OIDC
oauth = OAuth()

# Server-side session storage (filesystem backend)
sess = Session()


def init_extensions(app):
    """Initialize Flask extensions with the application."""
    csrf.init_app(app)

    # Build the cachelib filesystem backend at runtime so importing the config
    # never creates directories. Keeps session data off the client cookie.
    if app.config.get('SESSION_TYPE') == 'cachelib' and not app.config.get('SESSION_CACHELIB'):
        from cachelib.file import FileSystemCache
        app.config['SESSION_CACHELIB'] = FileSystemCache(
            cache_dir=app.config['SESSION_DIR'], threshold=500
        )
    sess.init_app(app)

    oauth.init_app(app)

    # Register OIDC provider if configured
    if app.config.get('OIDC_ISSUER'):
        oauth.register(
            name='oidc',
            server_metadata_url=app.config.get('OIDC_ISSUER'),
            client_id=app.config.get('OIDC_CLIENT_ID'),
            client_secret=app.config.get('OIDC_CLIENT_SECRET'),
            client_kwargs={'scope': 'openid email profile offline_access'},
        )
