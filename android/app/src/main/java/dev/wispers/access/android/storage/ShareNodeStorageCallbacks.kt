package dev.wispers.access.android.storage

import dev.wispers.connect.storage.NodeStorageCallbacks

internal class ShareNodeStorageCallbacks(
    private val dao: ShareDao,
    private val shareId: String,
) : NodeStorageCallbacks {
    override fun loadRootKey(): ByteArray? = dao.getRootKey(shareId)
    override fun saveRootKey(key: ByteArray) = dao.setRootKey(shareId, key)
    override fun deleteRootKey() = dao.clearRootKey(shareId)
    override fun loadRegistration(): ByteArray? = dao.getRegistration(shareId)
    override fun saveRegistration(data: ByteArray) = dao.setRegistration(shareId, data)
    override fun deleteRegistration() = dao.clearRegistration(shareId)
}
