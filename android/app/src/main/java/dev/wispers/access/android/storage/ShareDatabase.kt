package dev.wispers.access.android.storage

import android.content.Context
import androidx.room.Database
import androidx.room.Room
import androidx.room.RoomDatabase
import net.zetetic.database.sqlcipher.SupportOpenHelperFactory

@Database(entities = [ShareEntity::class], version = 1, exportSchema = true)
internal abstract class ShareDatabase : RoomDatabase() {

    abstract fun shareDao(): ShareDao

    companion object {
        fun create(context: Context, passphrase: ByteArray): ShareDatabase {
            System.loadLibrary("sqlcipher")
            var builder = Room.databaseBuilder(
                context.applicationContext,
                ShareDatabase::class.java,
                "shares.db",
            )
            return builder
                .openHelperFactory(SupportOpenHelperFactory(passphrase))
                .build()
        }
    }
}
