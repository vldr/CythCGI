for run in {1..10000}; do
  SCRIPT_FILENAME=examples/db.cy cgi-fcgi -bind -connect 127.0.0.1:1237
done