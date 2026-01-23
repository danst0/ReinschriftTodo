#!/usr/bin/env python3
"""Entry point for the Flask application."""

import os
import sys

# Add the webapp directory to the path for imports
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from app import create_app

app = create_app()

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=5000, threaded=True)
