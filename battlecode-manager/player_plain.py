import os
import psutil
import subprocess
import threading
import sys
from threading import Timer
import select

from player_abstract import AbstractPlayer


class PlainPlayer(AbstractPlayer):
    def __init__(self, socket_file, working_dir, local_dir=None,
                 player_key="", player_mem_limit=256, player_cpu=20):

        super().__init__(socket_file, working_dir, local_dir, None, None, player_key, player_mem_limit, player_cpu)

        self.paused = False
        self.streaming = False
        self.process = None

    def stream_logs(self, stdout=True, stderr=True, line_action=lambda line: print(line.decode())):
        assert not self.streaming
        self.streaming = True
        if stdout:
            threading.Thread(target=self._stream_logs, args=(self.process.stdout, line_action)).start()
        if stderr:
            threading.Thread(target=self._stream_logs, args=(self.process.stderr, line_action)).start()

    def _stream_logs(self, stream, line_action):
        for line in stream:
            if self.process is None:
                return
            line_action(line)

    def start(self):
        if sys.platform == 'win32':
            args = [os.path.join(self.working_dir, 'run.bat')]
            # things break otherwise
            env = dict(os.environ)
        else:
            args = ['sh', os.path.join(self.working_dir, 'run.sh')]
            # Path needs to be passed through, otherwise some compilers (e.g gcc) can get confused and not find things
            env = {'PATH': os.environ['PATH']}

        env['PLAYER_KEY'] = str(self.player_key)
        env['RUST_BACKTRACE'] = '1'
        env['BC_PLATFORM'] = self._detect_platform()

        if isinstance(self.socket_file, tuple):
            # tcp port
            env['TCP_PORT'] = str(self.socket_file[1])
        else:
            env['SOCKET_FILE'] = self.socket_file

        cwd = self.working_dir
        self.process = psutil.Popen(args, env=env, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=-1)

    def pause(self):
        if not self.paused:
            self.paused = True
            suspend(self.process)

    def unpause(self, timeout=None):
        if self.paused:
            resume(self.process)
            self.paused = False

    def destroy(self):
        if self.process is not None:
            tmp = self.process
            # This will signal to the log thread that everything is going to be shut down
            # and ignore any future messages. In particular bash may log something like 'Terminated: <PID>'
            # which would pollute the output of this script.
            self.process = None
            reap(tmp)
            self.process = None
        super().destroy()

def reap(process, timeout=3):
    "Tries hard to terminate and ultimately kill all the children of this process."
    def on_terminate(proc):
        pass
        # print("process {} terminated with exit code {}".format(proc.pid, proc.returncode))

    try:
        procs = process.children(recursive=True)
        # send SIGTERM
        for p in procs:
            p.terminate()
        gone, alive = psutil.wait_procs(procs, timeout=timeout, callback=on_terminate)
        if alive:
            # send SIGKILL
            for p in alive:
                p.kill()
            gone, alive = psutil.wait_procs(alive, timeout=timeout, callback=on_terminate)
            if alive:
                # give up
                for p in alive:
                    print("process {} survived SIGKILL; giving up" % p.pid)

        process.kill()
    except:
        print("Killing failed; assuming process exited early.")

import time
def suspend(process):
    s = time.time()
    procs = process.children(recursive=True)
    # to enterprising players reading this code:
    # yes, it is possible to escape the pausing using e.g. `nohup` when running without docker.
    # however, that won't work while running inside docker. Sorry.
    for p in procs:
        try:
            p.suspend()
        except:
            pass
    try:
        p.suspend()
    except:
        pass
    print('suspension took:', time.time() - s)

def resume(process):
    s = time.time()
    procs = process.children(recursive=True)
    for p in procs:
        try:
            p.resume()
        except:
            pass
    try:
        p.resume()
    except:
        pass
    print('resumption took:', time.time() - s)

