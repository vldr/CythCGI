for run in {1..10000}; do
  SCRIPT_FILENAME=examples/index.cy cgi-fcgi -bind -connect 127.0.0.1:1237 | grep Interval
done