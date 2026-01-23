"""Authentication routes blueprint - login, logout, OIDC."""

import os
from flask import Blueprint, render_template, request, redirect, url_for, session, flash

from translations import TRANSLATIONS
from app.extensions import oauth

auth_bp = Blueprint('auth', __name__)


def get_locale() -> str:
    """Get current locale from session or request."""
    if 'lang' in session:
        return session['lang']
    accept_languages = request.accept_languages.best_match(TRANSLATIONS.keys())
    return accept_languages or 'de'


@auth_bp.route('/login', methods=['GET', 'POST'])
def login():
    """Handle login page and form submission."""
    show_password_login = bool(os.environ.get('APP_USER') and os.environ.get('APP_PASSWORD'))
    show_oidc_login = bool(os.environ.get('OIDC_ISSUER') and os.environ.get('OIDC_CLIENT_ID'))

    if request.method == 'POST' and show_password_login:
        username = request.form.get('username')
        password = request.form.get('password')
        if username == os.environ.get('APP_USER') and password == os.environ.get('APP_PASSWORD'):
            session.permanent = True
            session['logged_in'] = True
            return redirect(url_for('main.index'))
        else:
            lang = get_locale()
            flash(TRANSLATIONS.get(lang, TRANSLATIONS['de'])['invalid_credentials'])

    return render_template('login.html',
                          show_password_login=show_password_login,
                          show_oidc_login=show_oidc_login)


@auth_bp.route('/login/oidc')
def login_oidc():
    """Initiate OIDC login flow."""
    redirect_uri = os.environ.get('OIDC_REDIRECT_URI') or url_for('auth.authorize', _external=True)
    return oauth.oidc.authorize_redirect(redirect_uri)


@auth_bp.route('/authorize')
def authorize():
    """Handle OIDC authorization callback."""
    token = oauth.oidc.authorize_access_token()
    user_info = token.get('userinfo')
    if not user_info:
        user_info = oauth.oidc.userinfo(token=token)

    email = user_info.get('email', '').lower()
    allowed_user = os.environ.get('OIDC_ALLOWED_USER', '').lower()

    if allowed_user and email == allowed_user:
        session.permanent = True
        session['logged_in'] = True
        return redirect(url_for('main.index'))
    else:
        lang = get_locale()
        flash(TRANSLATIONS.get(lang, TRANSLATIONS['de'])['unauthorized_user'])
        return redirect(url_for('auth.login'))


@auth_bp.route('/logout')
def logout():
    """Handle logout."""
    session.pop('logged_in', None)
    return redirect(url_for('auth.login'))
