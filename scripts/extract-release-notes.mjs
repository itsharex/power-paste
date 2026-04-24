import fs from 'node:fs/promises'
import path from 'node:path'

const CHANGELOG_FILE = 'Changelog.md'
const VERSION_HEADER_PREFIX = '## '

function resolveTag() {
  const explicitTag = process.argv[2]?.trim()
  if (explicitTag) {
    return explicitTag
  }

  const envTag = process.env.GITHUB_REF_NAME?.trim()
  if (envTag) {
    return envTag
  }

  throw new Error('缺少版本标签，请通过命令参数或 GITHUB_REF_NAME 传入，例如 v0.3.6')
}

async function readChangelog() {
  const filePath = path.resolve(process.cwd(), CHANGELOG_FILE)
  return {
    filePath,
    content: await fs.readFile(filePath, 'utf8'),
  }
}

function extractReleaseNotes(content, tag) {
  const lines = content.replace(/\r\n/g, '\n').split('\n')
  const header = `${VERSION_HEADER_PREFIX}${tag}`
  const startIndex = lines.findIndex((line) => line.trim() === header)

  if (startIndex === -1) {
    throw new Error(`在 ${CHANGELOG_FILE} 中未找到版本 ${tag} 的标题 ${header}`)
  }

  const notes = []

  for (let index = startIndex + 1; index < lines.length; index += 1) {
    const currentLine = lines[index]
    const trimmedLine = currentLine.trim()

    if (trimmedLine.startsWith(VERSION_HEADER_PREFIX) && trimmedLine !== '---') {
      break
    }

    if (trimmedLine === '---') {
      break
    }

    notes.push(currentLine)
  }

  const normalizedNotes = notes.join('\n').trim()
  if (!normalizedNotes) {
    throw new Error(`版本 ${tag} 的更新说明为空，请先补充 ${CHANGELOG_FILE}`)
  }

  return normalizedNotes
}

async function writeGithubEnv(notes) {
  if (!process.env.GITHUB_ENV) {
    return
  }

  // GitHub Actions 通过 heredoc 语法接收多行环境变量。
  const payload = `UPDATE_NOTES<<EOF\n${notes}\nEOF\n`
  await fs.appendFile(process.env.GITHUB_ENV, payload, 'utf8')
}

async function main() {
  const tag = resolveTag()
  const { content } = await readChangelog()
  const notes = extractReleaseNotes(content, tag)

  await writeGithubEnv(notes)
  process.stdout.write(`${notes}\n`)
}

main().catch((error) => {
  process.stderr.write(`${error.message}\n`)
  process.exitCode = 1
})
