"""gunicorn settings, loaded automatically from the working directory (/app).

They live here rather than on the command line because the production compose
file overrides the image's CMD; a file in the image applies either way.
"""

# gunicorn 26 opens a control socket under $HOME by default. In the container
# $HOME is "/", which the app user cannot write, so every start logged
# "Control server error: Permission denied: '/.gunicorn'". Nothing uses the
# control interface. Older gunicorn versions ignore this unknown setting.
control_socket_disable = True
