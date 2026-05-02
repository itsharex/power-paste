import { computed, onUnmounted, ref } from 'vue'
import {
  getLanReceiverState,
  onLanReceiverStatus,
  startLanReceiver,
  stopLanReceiver,
} from '../services/tauriApi'

function formatError(error) {
  if (typeof error === 'string') {
    return error
  }
  if (error && typeof error.message === 'string') {
    return error.message
  }
  return String(error || '')
}

export function useLanReceiver({ t }) {
  const showLanReceiver = ref(false)
  const lanReceiverState = ref({
    running: false,
    url: null,
    qrSvg: null,
    expiresAt: null,
    lastStatus: null,
  })
  const lanReceiverBusy = ref(false)
  const lanReceiverError = ref('')
  const now = ref(Date.now())
  let unlisten = null
  let timer = null

  const expiresInSeconds = computed(() => {
    const expiresAt = Number(lanReceiverState.value?.expiresAt || 0)
    if (!expiresAt) {
      return 0
    }
    return Math.max(0, Math.ceil((expiresAt - now.value) / 1000))
  })

  const expiresInLabel = computed(() => {
    const seconds = expiresInSeconds.value
    const minutes = Math.floor(seconds / 60)
    const rest = String(seconds % 60).padStart(2, '0')
    return `${minutes}:${rest}`
  })

  const statusLabel = computed(() => {
    const status = lanReceiverState.value?.lastStatus
    if (!status) {
      return lanReceiverState.value.running ? t('lanReceiverReady') : t('lanReceiverStopped')
    }
    if (status.kind === 'success') {
      if (status.receivedKind === 'image') {
        return t('lanReceiverReceivedImage')
      }
      return t('lanReceiverReceivedText')
    }
    if (status.kind === 'processing') {
      return t('lanReceiverProcessingImage')
    }
    return status.message || t('lanReceiverFailed')
  })

  function applyState(next) {
    lanReceiverState.value = {
      running: Boolean(next?.running),
      url: next?.url || null,
      qrSvg: next?.qrSvg || null,
      expiresAt: next?.expiresAt || null,
      lastStatus: next?.lastStatus || null,
    }
  }

  async function setupLanReceiverListener() {
    if (unlisten) {
      return
    }
    unlisten = await onLanReceiverStatus((event) => {
      if (event?.payload) {
        applyState(event.payload)
      }
    })
  }

  async function refreshLanReceiverState() {
    applyState(await getLanReceiverState())
  }

  async function openLanReceiver() {
    showLanReceiver.value = true
    lanReceiverError.value = ''
    lanReceiverBusy.value = true
    try {
      await setupLanReceiverListener()
      applyState(await startLanReceiver())
    } catch (error) {
      lanReceiverError.value = formatError(error)
    } finally {
      lanReceiverBusy.value = false
    }
  }

  async function closeLanReceiver() {
    lanReceiverError.value = ''
    lanReceiverBusy.value = true
    try {
      applyState(await stopLanReceiver())
      showLanReceiver.value = false
    } catch (error) {
      lanReceiverError.value = formatError(error)
    } finally {
      lanReceiverBusy.value = false
    }
  }

  timer = window.setInterval(() => {
    now.value = Date.now()
    if (lanReceiverState.value.running && expiresInSeconds.value <= 0) {
      refreshLanReceiverState()
    }
  }, 1000)

  onUnmounted(() => {
    if (timer) {
      window.clearInterval(timer)
    }
    unlisten?.()
  })

  return {
    closeLanReceiver,
    expiresInLabel,
    lanReceiverBusy,
    lanReceiverError,
    lanReceiverState,
    openLanReceiver,
    refreshLanReceiverState,
    showLanReceiver,
    statusLabel,
  }
}
