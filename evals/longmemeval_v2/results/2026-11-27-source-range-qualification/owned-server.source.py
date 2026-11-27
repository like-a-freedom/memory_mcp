import os,sys
from pathlib import Path
Path('/tmp/memory-quality-owned-mlx-reader.pid').write_text(str(os.getpid()))
os.execv(sys.executable,[sys.executable,'-m','mlx_lm','server','--model','/Users/solovey/.cache/lm-studio/models/lmstudio-community/Qwen3.8-27B-MLX-4bit','--host','127.0.0.1','--port','18767','--max-tokens','384','--temp','0','--chat-template-args','{"enable_thinking":false}','--decode-concurrency','1','--prompt-concurrency','1'])
