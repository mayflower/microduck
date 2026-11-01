#!/usr/bin/env python3
"""Feed identical paced PCM to robotd's simulated microphone and optional speakers."""

import argparse
import json
import subprocess
import sys
import time
import wave
from pathlib import Path


def replay(
    wav_path,
    silence_file=None,
    play_audio=False,
    playback_log=None,
    playback_error=None,
    clock_file=None,
):
    with wave.open(str(wav_path)) as source:
        if source.getparams()[:3] != (1, 2, 16000):
            raise ValueError("expected mono 16 kHz PCM16 WAV")
        data = source.readframes(source.getnframes())
    if not data:
        raise ValueError("empty audio file")
    player = None
    try:
        if play_audio:
            with playback_log.open("ab") as log:
                player = subprocess.Popen(
                    [
                        "aplay",
                        "-q",
                        "-t",
                        "raw",
                        "-f",
                        "S16_LE",
                        "-r",
                        "16000",
                        "-c",
                        "1",
                        "--buffer-time=80000",
                        "--period-time=20000",
                    ],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.DEVNULL,
                    stderr=log,
                )
        cursor = 0
        sent_samples = 0
        start = time.monotonic()
        if clock_file is not None:
            clock_file.write_text(
                json.dumps(
                    {
                        "source_start_monotonic_ns": round(start * 1e9),
                        "sample_rate": 16000,
                        "estimated_playback_latency_ms": 80 if play_audio else 0,
                    }
                )
                + "\n"
            )
        while True:
            block = data[cursor : cursor + 1024]
            cursor = (cursor + len(block)) % len(data)
            if silence_file is not None and silence_file.exists():
                block = bytes(len(block))
            if player is not None:
                if player.poll() is not None:
                    raise RuntimeError("speaker playback stopped; see playback.log")
                player.stdin.write(block)
                player.stdin.flush()
            sys.stdout.buffer.write(block)
            sys.stdout.buffer.flush()
            sent_samples += len(block) // 2
            time.sleep(max(0, start + sent_samples / 16000 - time.monotonic()))
    except BrokenPipeError:
        # Capture is routinely closed when robotd disables or restarts listening.
        # A living capture pipe distinguishes that from a failed speaker process.
        if (
            player is not None
            and player.poll() not in (None, 0)
            and playback_error is not None
        ):
            playback_error.write_text("speaker playback failed; see playback.log\n")
    except Exception as error:
        if playback_error is not None:
            playback_error.write_text(str(error) + "\n")
        raise
    finally:
        if player is not None:
            try:
                player.stdin.close()
            except BrokenPipeError:
                pass
            try:
                player.wait(timeout=1)
            except subprocess.TimeoutExpired:
                player.terminate()
                player.wait(timeout=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wav", type=Path)
    parser.add_argument("--silence-file", type=Path)
    parser.add_argument("--play-audio", action="store_true")
    parser.add_argument("--playback-log", type=Path, default=Path("playback.log"))
    parser.add_argument("--playback-error", type=Path)
    parser.add_argument("--clock-file", type=Path)
    args = parser.parse_args()
    replay(
        args.wav,
        args.silence_file,
        args.play_audio,
        args.playback_log,
        args.playback_error,
        args.clock_file,
    )


if __name__ == "__main__":
    main()
