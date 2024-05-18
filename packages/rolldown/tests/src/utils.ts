import { RollupOutput, RolldownOutputChunk } from '../../src'
import nodePath from 'node:path'
import nodeUrl from 'node:url'
import assert from 'node:assert'
import { workspaceRoot } from '@rolldown/testing'

export function getOutputChunkNames(output: RollupOutput) {
  return output.output
    .filter((chunk) => chunk.type === 'chunk')
    .map((chunk) => chunk.fileName)
    .sort()
}

export function getOutputChunk(output: RollupOutput): RolldownOutputChunk[] {
  return output.output.filter(
    (chunk) => chunk.type === 'chunk',
  ) as RolldownOutputChunk[]
}

export function getOutputFileNames(output: RollupOutput) {
  return output.output.map((chunk) => chunk.fileName).sort()
}

/**
 *
 * @returns The absolute path to the `${WORKSPACE}/packages/rolldown` directory
 */
export function projectDir(...joined: string[]) {
  return workspaceRoot('packages/rolldown', ...joined)
}

/**
 *
 * @returns The absolute path to the `${WORKSPACE}/packages/rolldown/tests` directory
 */
export function testsDir(...joined: string[]) {
  return projectDir('tests', ...joined)
}

assert.deepEqual(testsDir().split(nodePath.sep).slice(-4), [
  'rolldown',
  'packages',
  'rolldown',
  'tests',
])
