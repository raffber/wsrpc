import json
from flask import Flask, request
import sys

app = Flask(__name__)


@app.route("/", methods=["GET", "POST"])
def hello():
    req = json.loads(request.data)
    if req == {"Shutdown": None}:
        sys.exit(0)
    return req


if __name__ == "__main__":
    app.run(port=7480)
