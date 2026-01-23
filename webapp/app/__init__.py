"""Flask application factory."""

import logging
import os

from flask import Flask
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

    # Register blueprints
    register_blueprints(app)

    # Register context processors
    register_context_processors(app)

    # Register error handlers
    register_error_handlers(app)

    return app


def register_blueprints(app: Flask) -> None:
    """Register all application blueprints."""
    from app.routes.main import main_bp
    from app.routes.auth import auth_bp
    from app.routes.todo import todo_bp
    from app.routes.api import api_bp

    app.register_blueprint(main_bp)
    app.register_blueprint(auth_bp)
    app.register_blueprint(todo_bp)
    app.register_blueprint(api_bp, url_prefix='/api')


def register_context_processors(app: Flask) -> None:
    """Register Jinja2 context processors."""
    from flask import session, request
    from translations import TRANSLATIONS

    @app.context_processor
    def inject_translations():
        """Inject translation dictionary into templates."""
        lang = get_locale()
        return {
            't': TRANSLATIONS.get(lang, TRANSLATIONS['de']),
            'current_lang': lang
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
