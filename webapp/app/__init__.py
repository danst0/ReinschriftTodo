"""Flask application factory."""

import logging
import os
import time

import requests as http_requests
from flask import Flask, session, redirect, url_for, request
from werkzeug.middleware.proxy_fix import ProxyFix

from app.config import config
from app.extensions import init_extensions


def create_app(config_name: str | None = None) -> Flask:
    """Create and configure the Flask application.

    Args:
        config_name: Name of configuration to use ('development', 'production', 'testing').
                    Defaults to FLASK_ENV environment variable or 'development'.

    Returns:
        Configured Flask application instance.
    """
    if config_name is None:
        config_name = os.environ.get('FLASK_ENV', 'development')

    app = Flask(__name__,
                template_folder='../templates',
                static_folder='../static')

    # Load configuration
    app.config.from_object(config.get(config_name, config['default']))

    # Configure logging
    log_level = getattr(logging, app.config.get('LOG_LEVEL', 'INFO'), logging.INFO)
    logging.basicConfig(
        level=log_level,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )

    # Honor reverse proxy headers (e.g., X-Forwarded-Proto)
    app.wsgi_app = ProxyFix(
        app.wsgi_app,
        x_for=1, x_proto=1, x_host=1, x_port=1, x_prefix=1
    )

    # Initialize extensions
    init_extensions(app)

    # Register OIDC token refresh hook
    register_token_refresh(app)

    # Register blueprints
    register_blueprints(app)

    # Register context processors
    register_context_processors(app)

    # Register error handlers
    register_error_handlers(app)

    return app


def register_token_refresh(app: Flask) -> None:
    """Register a before_request hook that refreshes expired OIDC tokens."""
    logger = logging.getLogger(__name__)

    @app.before_request
    def refresh_oidc_token():
        # Skip if no OIDC refresh token in session (password login or unauthenticated)
        if 'oidc_refresh_token' not in session:
            return

        # Skip auth routes to avoid redirect loops
        if request.endpoint and request.endpoint.startswith('auth.'):
            return

        # Skip static files
        if request.endpoint == 'static':
            return

        expires_at = session.get('oidc_expires_at')
        if expires_at is None:
            return

        # Check if token is expired or will expire within 60 seconds
        if time.time() < (expires_at - 60):
            return

        # Token is expired — attempt refresh
        issuer = app.config.get('OIDC_ISSUER')
        client_id = app.config.get('OIDC_CLIENT_ID')
        client_secret = app.config.get('OIDC_CLIENT_SECRET')
        refresh_token = session.get('oidc_refresh_token')

        if not all([issuer, client_id, refresh_token]):
            logger.warning('Missing OIDC config for token refresh, clearing session')
            session.clear()
            return redirect(url_for('auth.login'))

        # Discover the token endpoint from the OIDC provider metadata
        try:
            metadata = http_requests.get(issuer, timeout=10).json()
            token_endpoint = metadata.get('token_endpoint')
        except Exception:
            logger.error('Failed to fetch OIDC metadata for token refresh')
            session.clear()
            return redirect(url_for('auth.login'))

        if not token_endpoint:
            logger.error('No token_endpoint in OIDC metadata')
            session.clear()
            return redirect(url_for('auth.login'))

        # Exchange the refresh token for a new access token
        try:
            resp = http_requests.post(token_endpoint, data={
                'grant_type': 'refresh_token',
                'refresh_token': refresh_token,
                'client_id': client_id,
                'client_secret': client_secret,
            }, timeout=10)
            resp.raise_for_status()
            token_data = resp.json()
        except Exception:
            logger.error('OIDC token refresh failed')
            session.clear()
            return redirect(url_for('auth.login'))

        # Update session with new tokens
        session['oidc_token'] = token_data.get('access_token')
        session['oidc_expires_at'] = token_data.get('expires_at')
        # Some IdPs issue a new refresh token with each refresh
        if token_data.get('refresh_token'):
            session['oidc_refresh_token'] = token_data['refresh_token']

        logger.info('OIDC access token refreshed successfully')


def register_blueprints(app: Flask) -> None:
    """Register all application blueprints."""
    from app.extensions import csrf
    from app.routes.main import main_bp
    from app.routes.auth import auth_bp
    from app.routes.todo import todo_bp
    from app.routes.api import api_bp
    from app.routes.share import share_bp

    app.register_blueprint(main_bp)
    app.register_blueprint(auth_bp)
    app.register_blueprint(todo_bp)
    app.register_blueprint(api_bp, url_prefix='/api')
    app.register_blueprint(share_bp)

    # Share routes use the URL token as bearer auth; CSRF would only protect
    # against attackers who already have the token.
    csrf.exempt(share_bp)


def _get_version() -> str:
    """Read version from Cargo.toml (dev) or pyproject.toml (Docker fallback)."""
    import re
    from pathlib import Path

    # Try Cargo.toml (available in development, one level up from webapp/)
    cargo_path = Path(__file__).resolve().parent.parent.parent / 'Cargo.toml'
    if cargo_path.is_file():
        match = re.search(r'^version\s*=\s*"([^"]+)"', cargo_path.read_text(), re.MULTILINE)
        if match:
            return match.group(1)

    # Fallback: pyproject.toml (available in Docker)
    pyproject_path = Path(__file__).resolve().parent.parent / 'pyproject.toml'
    if pyproject_path.is_file():
        match = re.search(r'^version\s*=\s*"([^"]+)"', pyproject_path.read_text(), re.MULTILINE)
        if match:
            return match.group(1)

    return 'unknown'


def register_context_processors(app: Flask) -> None:
    """Register Jinja2 context processors."""
    from flask import session, request
    from translations import TRANSLATIONS

    app_version = _get_version()

    @app.context_processor
    def inject_translations():
        """Inject translation dictionary and version into templates."""
        lang = get_locale()
        return {
            't': TRANSLATIONS.get(lang, TRANSLATIONS['de']),
            'current_lang': lang,
            'app_version': app_version,
        }

    def get_locale() -> str:
        """Determine the current locale from session or request."""
        if 'lang' in session:
            return session['lang']

        accept_languages = request.accept_languages.best_match(TRANSLATIONS.keys())
        if accept_languages:
            return accept_languages

        return 'de'  # Default language


def register_error_handlers(app: Flask) -> None:
    """Register error handlers."""
    from flask_wtf.csrf import CSRFError
    from flask import render_template

    @app.errorhandler(CSRFError)
    def handle_csrf_error(e):
        return render_template('error.html', error='CSRF token missing or invalid'), 400

    @app.errorhandler(404)
    def handle_not_found(e):
        return render_template('error.html', error='Page not found'), 404

    @app.errorhandler(500)
    def handle_internal_error(e):
        return render_template('error.html', error='Internal server error'), 500
