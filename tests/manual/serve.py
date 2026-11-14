import sys, uvicorn
from athar import server as srv
print(f"URL http://127.0.0.1:8765/?token={srv.TOKEN}", flush=True)
srv.library().add_folder(sys.argv[1])
uvicorn.run(srv.app, host="127.0.0.1", port=8765, log_level="warning")
