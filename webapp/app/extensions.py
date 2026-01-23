"""Flask extensions initialization."""

from flask_wtf.csrf import CSRFProtect
from authlib.integrations.flask_client import OAuth

# CSRF Protection
csrf = CSRFProtect()

# OAuth client for OIDC
oauth = OAuth()


def init_extensions(app):
    """Initialize Flask extensions with the application."""
    csrf.init_app(app)
    oauth.init_app(app)

    # Register OIDC provider if configured
    if app.config.get('OIDC_ISSUER'):
        oauth.register(
            name='oidc',
            server_metadata_url=app.config.get('OIDC_ISSUER'),
            client_id=app.config.get('OIDC_CLIENT_ID'),
            client_secret=app.config.get('OIDC_CLIENT_SECRET'),
            client_kwargs={'scope': 'openid email profile'},
        )
