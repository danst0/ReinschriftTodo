FROM python:3.11-slim

WORKDIR /app

# Install dependencies
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy application code
COPY . .

# Set Python path to include app directory
ENV PYTHONPATH=/app

# Increase timeout to outlive AI calls (requests use up to 45s)
# Use the new modular app factory (run.py) or legacy app.py
CMD ["gunicorn", "-w", "4", "-b", "0.0.0.0:5000", "--timeout", "90", "run:app"]
