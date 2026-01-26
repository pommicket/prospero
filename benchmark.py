#!/usr/bin/env python3
import sys
import os
import shutil
import random
import time

trials = 100

try:
	os.mkdir('benchmarks')
except FileExistsError:
	pass

def print_usage_and_exit() -> None:
	print('''Usage:
benchmark.py register <name>
benchmark.py compare <name1> <name2>''') 
	exit()	

def register_binary(name: str) -> None:
	out_path = 'benchmarks/' + name
	if os.path.exists(out_path):
		if not input(out_path + ' exists. Delete it [y/n]? ').lower().startswith('y'):
			print('Aborted.')
			return
		os.remove(out_path)
	shutil.copyfile('target/release/prospero', out_path)
	os.chmod(out_path, 0o755)
	print(name + ' registered.')

def compare_binaries(name1: str, name2: str) -> None:
	whiches = [False for i in range(trials)] + [True for i in range(trials)]
	random.shuffle(whiches)
	results = [[], []]
	for i,which in enumerate(whiches):
		if i % 20 == 0:
			print(i,'/',trials*2)
		path = 'benchmarks/' + [name1, name2][int(which)]
		start_time = time.time()
		if os.system(path):
			print('Program ' + which + 'failed. Aborting.')
			exit()
		end_time = time.time()
		results[int(which)].append(end_time - start_time)
	results[0].sort()
	results[1].sort()
	print(name1+':'+' '*max(0,len(name2)-len(name1)),results[0][trials//2])
	print(name2+':'+' '*max(0,len(name1)-len(name2)),results[1][trials//2])

if len(sys.argv) < 2:
	print_usage_and_exit()
verb = sys.argv[1]
if verb in ['help', '-h', '--help']:
	print_usage_and_exit()
if verb == 'register':
	if len(sys.argv) != 3: print_usage_and_exit()
	os.system('cargo b --release')
	register_binary(sys.argv[2])
elif verb == 'compare':
	if len(sys.argv) != 4: print_usage_and_exit()
	compare_binaries(sys.argv[2], sys.argv[3])
