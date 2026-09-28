package dev.wispers.access.android.storage

import android.content.Context
import androidx.room.Database
import androidx.room.Room
import androidx.room.RoomDatabase
import net.zetetic.database.sqlcipher.SupportOpenHelperFactory

/**
 * What the app keeps beside the SDK's own store: the SDK's secrets, behind
 * SQLCipher so a share's key material sits encrypted at rest, and the UI-only
 * extras the SDK does not track, when a share was last reached and the icons
 * harvested per app. The SDK's metadata (shares, their apps, their state)
 * lives in the SDK's data directory, not here.
 *
 * A new file next to the pre-SDK app's `shares.db`, which is left untouched.
 */
@Database(
    entities = [SecretEntity::class, ShareActivityEntity::class, AppIconEntity::class],
    version = 1,
    exportSchema = true,
)
internal abstract class ShareDatabase : RoomDatabase() {

    abstract fun secretDao(): SecretDao

    abstract fun extrasDao(): ExtrasDao

    companion object {
        fun create(context: Context, passphrase: ByteArray): ShareDatabase {
            System.loadLibrary("sqlcipher")
            return Room.databaseBuilder(
                context.applicationContext,
                ShareDatabase::class.java,
                "wispers-access.db",
            )
                .openHelperFactory(SupportOpenHelperFactory(passphrase))
                .build()
        }
    }
}
