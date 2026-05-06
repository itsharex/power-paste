<script setup>
defineProps({
  cancelLabel: { type: String, required: true },
  confirmLabel: { type: String, required: true },
  message: { type: String, required: true },
  show: { type: Boolean, required: true },
  title: { type: String, required: true },
})

const emit = defineEmits(['close', 'confirm'])
</script>

<template>
  <div v-if="show" class="modal-backdrop" @click="emit('close')">
    <section class="confirm-modal" @click.stop>
      <header class="modal-header confirm-modal-header">
        <div class="modal-title-block">
          <h2>{{ title }}</h2>
        </div>
      </header>

      <p class="confirm-modal-message">{{ message }}</p>

      <footer class="modal-footer confirm-modal-footer">
        <button class="ghost" type="button" @click="emit('close')">
          {{ cancelLabel }}
        </button>
        <button class="primary" type="button" @click="emit('confirm')">
          {{ confirmLabel }}
        </button>
      </footer>
    </section>
  </div>
</template>
