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

def compare_binaries(short_name1: str, short_name2: str) -> None:
	names = os.listdir('benchmarks')
	names1 = [name for name in names if name.startswith(short_name1)]
	names2 = [name for name in names if name.startswith(short_name2)]
	if len(names1) > 1:
		print(short_name1, 'is ambiguous. Could be any of:', names1)
	if len(names2) > 1:
		print(short_name2, 'is ambiguous. Could be any of:', names2)
	[name1] = names1
	[name2] = names2
	print('Comparing', name1, 'vs', name2, '...')
	whiches = [False for i in range(trials)] + [True for i in range(trials)]
	random.shuffle(whiches)
	results = [[], []]
	for i,which in enumerate(whiches):
		if i % 20 == 0:
			print(i,'/',trials*2)
		name = [name1, name2][int(which)]
		path = 'benchmarks/' + name
		start_time = time.time()
		if os.system(path):
			print(f'Program {name} failed. Aborting.')
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
