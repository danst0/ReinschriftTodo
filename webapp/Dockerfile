FROM python:3.11-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . .

# Increase timeout to outlive AI calls (requests use up to 45s)
CMD ["gunicorn", "-w", "4", "-b", "0.0.0.0:5000", "--timeout", "90", "app:app"]
