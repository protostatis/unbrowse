FROM python:3.12-slim

ENV PYTHONUNBUFFERED=1

RUN pip install --no-cache-dir pyunbrowser==0.0.13

ENTRYPOINT ["unbrowser", "--mcp"]
